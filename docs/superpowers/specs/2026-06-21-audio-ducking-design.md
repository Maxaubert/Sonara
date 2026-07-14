# Audio Ducking ("Audio Control") Design

**Status:** Approved design, ready for implementation plan.
**Date:** 2026-06-21
**Platform:** Windows-only (matches the rest of Sonara).

## Goal

A user-toggleable feature - working name **"audio control"** - that lowers the
volume of all *other* applications' audio on the PC while Sonara's TTS is
speaking, then restores it when Sonara goes quiet. Use case: watching a movie,
the screen reader speaks → the movie ducks → Sonara reads → the movie returns to
its original volume. Default **off** (opt-in), persisted across restarts.

## Behavioral requirements (decided)

1. **Hold, don't flap.** Duck once when speech begins and keep other audio
   lowered through the *entire* run of speech - across every sentence, every cue,
   AND every queued session - restoring only when Sonara is **completely idle**.
   No bouncing the movie's volume between sentences.
2. **Configurable duck level.** A `duck_level` (0-100, target % volume for other
   apps while ducked), default **20**. Set via a command, like `rate`/`verbosity`.
3. **Command toggle, no hotkey.** Enable/disable via a slash command only;
   persisted in config. It's a set-and-forget preference, not a mid-session mash.
4. **Never duck ourselves.** Sonara's own audio (daemon speech + its earcon
   helper processes) is always excluded.
5. **Never get stuck.** Other apps' volume must be restored even on a daemon
   crash. Ducking is best-effort: it must never break or delay speech.

## Approach (chosen)

**Per-session ducking via `pycaw`.** `pycaw` is the standard Python wrapper over
the Windows Core Audio session API (`IAudioSessionManager2` /
`ISimpleAudioVolume`); it is pure-Python on top of `comtypes`, which is already
installed on the daemon's interpreter. It is the only approach that lowers
*everyone else* without touching Sonara's own playback (rejected alternatives:
raw `comtypes` COM boilerplate - fragile, no benefit; lowering the master
endpoint volume - also lowers Sonara's own speech).

## Architecture

### New module: `src/sonara/platform/windows/ducking.py`

Exposes an `AudioDucker` class. Windows-only `pycaw`/`comtypes` imports are lazy
(inside methods) so the module imports anywhere (tests, non-Windows).

```
class AudioDucker:
    def duck(self, exclude_pids: set[int], level: int) -> None:
        """Lower every active audio session NOT owned by exclude_pids to
        level% of full. Records each session's prior volume scalar for restore.
        Idempotent: a no-op if already ducked. Best-effort; never raises."""

    def restore(self) -> None:
        """Set every recorded session back to its saved volume; clear state.
        Idempotent; never raises."""

    def is_ducked(self) -> bool: ...
```

- `duck()` enumerates sessions via pycaw's `AudioUtilities.GetAllSessions()`,
  skips any whose `Process` PID is in `exclude_pids`, reads each remaining
  session's `ISimpleAudioVolume` master scalar, and sets the scalar to
  `level / 100`. In memory it holds the live session objects plus their original
  scalars, so `restore()` can set them back directly.
- On duck it also writes the recovery file (see Crash recovery), keyed by
  **process identity** (pid + process name), not by the in-memory session object.
- `restore()` reverses the in-memory sessions and deletes the recovery file.

### Test seam

The daemon holds a `self.ducker` object, defaulting to the real `AudioDucker`
obtained from the Windows platform backend, but **injectable** for tests. Tests
pass a `FakeDucker` that records `duck()`/`restore()` calls. This mirrors the
existing tts/earcon/hotkey/supervisor backend seams.

The Windows `PlatformBackend` exposes the ducker (e.g. `backend.ducker`) so the
daemon obtains it the same way it obtains `tts`, `earcon`, `hotkey`.

### Hook points - the speak loop (`_speak_loop_once`)

Duck/restore live in the speak loop, the single place that knows global speaking
state. Per-utterance hooks are explicitly rejected (they would flap).

- **Duck:** immediately before speaking a real item (item is not `None` and not
  dropped by mute), if `config["audio_control"]` is on and `not ducker.is_ducked()`:
  `self.ducker.duck(self._duck_exclude_pids(), self._duck_level())`.
- **Restore:** in the idle path - when `next_item()` returns `None` (every
  channel drained, nothing playing) - if `ducker.is_ducked()`: `ducker.restore()`.

Because `next_item()` returns `None` only when all sessions are drained, the duck
holds across all sentences and all queued sessions and lifts exactly when Sonara
goes quiet (requirement 1).

`self._duck_exclude_pids()` returns `{os.getpid()} ∪ {live earcon helper PIDs}`.
The daemon's `Speaker` already tracks live earcon `Popen` handles in
`_earcon_procs`; expose their PIDs (e.g. `speaker.earcon_pids()`) so the daemon
can build the exclude set with no new dependency.

## Crash recovery (restore-guard)

Three layers ensure other apps' audio is never left ducked:

1. **Normal restore:** the idle-path `restore()`.
2. **Shutdown restore:** `daemon.stop()` calls `ducker.restore()` if ducked.
3. **Crash recovery via a state file:** `duck()` writes
   `~/.sonara/duck_state.json` = `{"sessions": [{"pid": N, "name": "app.exe",
   "original": 0.8}, ...]}`; `restore()` deletes it. On daemon **startup**
   (`main()`), if the file exists, a prior daemon died mid-duck → re-enumerate
   live sessions and best-effort restore any whose process identity (pid or name)
   matches a recorded entry (ignore apps that have since closed), then delete the
   file. Matching by process identity is necessary because the in-memory session
   objects do not survive a restart.
4. **Toggle-off restore:** turning `audio_control` off while ducked restores
   immediately rather than waiting for idle.

## Configuration

Added to `DEFAULTS` in `src/sonara/config.py`, persisted in `~/.sonara/config.json`:

- `"audio_control": false` - feature on/off (default off).
- `"duck_level": 20` - target % volume for other apps while ducked (clamped 0-100).

## Command + protocol wiring

Mirrors the existing `verbosity`/`rate` toggle pattern.

- **Protocol** (`src/sonara/protocol.py`): two new `MsgType`s -
  `SET_AUDIO_CONTROL` (payload on/off) and `SET_DUCK_LEVEL` (payload int 0-100).
- **Handlers** (`daemon.handle_message`): update `self.config`, call
  `save_config`, and speak a confirmation cue ("Audio control on." /
  "Audio control off." / "Duck level twenty percent."). `SET_AUDIO_CONTROL` with
  off, while ducked, calls `ducker.restore()` immediately. `SET_DUCK_LEVEL`
  clamps to 0-100; if currently ducked, re-applies at the new level.
- **CLI** (`src/sonara/cli.py`): `sonara audio-control on|off` and
  `sonara duck-level <0-100>`, each sending its message and printing the reply.
- **Slash commands:** `commands/audio-control.md` and `commands/duck-level.md`,
  routed through the launcher (`bin/sonara`) like the other command files, with
  the standard front-matter + "print the output" convention.
- **Dependency:** add `pycaw` to the `[windows]` extra in `pyproject.toml` and to
  `_ensure_speech_deps` so install provisions it. If absent at runtime, the
  feature self-disables (below).

## Error handling & edge cases

- **Best-effort, never breaks speech.** Every `AudioDucker` method is wrapped so
  any `pycaw`/COM exception is caught, logged once, and swallowed. If ducking
  fails, speech proceeds at full other-audio volume.
- **`pycaw` missing/unimportable:** `AudioDucker.duck()` logs a one-line hint and
  no-ops; the feature is effectively disabled, speech unaffected.
- **Session vanishes mid-duck** (app closed between enumerate and set): skipped,
  not fatal.
- **No other audio playing:** `duck()` finds nothing; harmless no-op.
- **App starts playing mid-reading:** not ducked until the next duck cycle (we
  duck once per batch, not per utterance, to avoid re-enumerating every
  sentence). Accepted and documented.
- **`duck_level` out of range:** clamped to 0-100.
- **Feature off:** the speak loop never calls `duck()`; zero overhead.

## Testing

Daemon tests use an injected `FakeDucker` (records calls); the real `AudioDucker`
is unit-tested with `pycaw` mocked at the COM seam, since live audio sessions are
not deterministic - the same strategy `tts.py` uses for WinRT.

1. **Duck on speak, only when enabled:** with `audio_control` on, the loop calls
   `duck()` before the first item; with it off, never.
2. **Hold / no-flap (the key behavior):** drive a multi-item, multi-session batch
   and assert exactly **one** `duck()` and **one** `restore()`, with `restore()`
   only after the loop reaches global idle.
3. **Toggle-off mid-speech:** `SET_AUDIO_CONTROL off` while ducked calls
   `restore()` immediately.
4. **Config/command:** `SET_AUDIO_CONTROL` / `SET_DUCK_LEVEL` update config,
   persist, clamp, and emit the cue.
5. **Crash recovery:** a `duck_state.json` present at startup triggers a restore
   sweep and the file is deleted.
6. **Exclude set:** `_duck_exclude_pids()` includes the daemon PID and live
   earcon helper PIDs.
7. **`AudioDucker` (pycaw mocked):** `duck()` lowers non-excluded sessions and
   records originals; `restore()` puts them back; failures are swallowed.

## Out of scope

- Cross-fade / gradual ramp of the duck (instant set is fine for v1).
- Per-app allow/deny lists (duck everyone but Sonara).
- A global hotkey for the toggle (command only, by decision).
- Non-Windows support.
- Fixing the separate mid-synthesis interrupt gap (tracked separately).
