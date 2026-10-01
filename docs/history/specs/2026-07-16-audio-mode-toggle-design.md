# Audio Behavior Mode (Off / Duck / Pause) - Design

**Status:** Approved for planning (2026-07-16)

**Goal:** Replace Sonara's boolean audio-ducking setting with a three-way audio
behavior mode - **Off**, **Duck**, **Pause** - and implement a new "Pause"
behavior that pauses other media while Sonara speaks instead of only lowering
its volume.

**Issue:** #90-follow-up (new issue to be filed at planning time).

---

## Global Constraints

- Python 3.14, standard library only in the daemon core. The pause backend uses
  the `winrt` package, which is already a project dependency (it drives Windows
  speech synthesis in `platform/windows/tts.py`).
- No em-dashes in code, comments, copy, or docs. Use en-dashes, commas, or
  rephrase.
- The pause backend is **Windows-only**, exactly like ducking. Non-Windows,
  missing `winrt`, or tests get a `NullPauser`.
- Every public method of the pause backend is **best-effort and never raises**:
  a failure to pause or resume must never break, block, or delay speech. This
  mirrors `AudioDucker`'s contract verbatim.
- All `winrt.*` imports in the pause backend are **lazy** (inside methods) so the
  module imports anywhere (tests, non-Windows).
- Settings mutations continue to flow through `daemon.handle_message` under the
  daemon lock (the existing webui `_dispatch` path). The page must not drift from
  CLI/daemon behavior.

---

## Behavior Summary

| Mode    | Other apps while Sonara speaks                                    |
|---------|-------------------------------------------------------------------|
| `off`   | Untouched. Play at full volume.                                   |
| `duck`  | All app volumes lowered to `duck_level`, restored when idle.       |
| `pause` | Real media (SMTC sessions that are *Playing*) paused, resumed when idle. Non-media audio (games, calls, notifications) left untouched. |

**Timing (all modes, identical to today's ducking):** engage at PLAYBACK start
(the `on_play` callback, after synthesis), disengage ONCE at global idle - never
between utterances. So media pauses once per speech batch and resumes once; no
flicker. Session-change announcements engage nothing (the #90 `on_play=None`
path for `session_change` items is untouched).

**Pause is pure (user decision):** in `pause` mode, audio that is not a pausable
SMTC media session (game SFX, a voice/video call, notification sounds) is left
alone. If the user wants everything quieted they use `duck`. There is no
duck-as-fallback hybrid.

**Resume is symmetric (user decision, SMTC targeted):** `pause()` records exactly
which sessions it paused (those that were *Playing*); `resume()` plays back only
those. A session the user had already paused themselves is never resumed by
Sonara.

---

## 1. Config & Protocol Model

### New config key
- `audio_mode`: `"off" | "duck" | "pause"`. **Default `"off"`.**
- `duck_level` stays as-is (int 0-100). Only meaningful in `duck` mode.

### Migration
On config load, if `audio_mode` is absent, derive it from the legacy
`audio_control` bool:
- `audio_control is True` -> `audio_mode = "duck"`
- otherwise -> `audio_mode = "off"`

Old configs keep working with no user action. The legacy `audio_control` key is
left in the config dict (not deleted) but is no longer authoritative; nothing in
the daemon reads it directly after migration.

### Daemon helpers (replace `_audio_control_on`)
- `_audio_mode() -> str` - returns the normalized mode string (defaults to
  `"off"` on any unexpected value).
- `_audio_duck_on() -> bool` - `_audio_mode() == "duck"`.
- `_audio_pause_on() -> bool` - `_audio_mode() == "pause"`.

### Protocol (`protocol.py`)
- Add `SET_AUDIO_MODE = "set_audio_mode"`. Message shape:
  `{"type": "set_audio_mode", "mode": "off" | "duck" | "pause"}`.
- Keep `SET_DUCK_LEVEL` unchanged.
- Keep `SET_AUDIO_CONTROL` as a **compat shim**: its handler maps
  `enabled=True -> audio_mode="duck"`, `enabled=False -> audio_mode="off"` by
  delegating to the same internal setter `SET_AUDIO_MODE` uses. This keeps any
  lingering CLI invocation working.
- `protocol.py` pinned-type test set is updated to include `SET_AUDIO_MODE`.

### CLI (`cli.py`)
- Keep `sonara audio-control on|off` (maps to mode via the shim).
- Add `sonara audio-mode off|duck|pause` sending `SET_AUDIO_MODE`.

---

## 2. Backend: MediaPauser (SMTC)

New file `src/sonara/platform/windows/pausing.py`, structured as a near-mirror of
`ducking.py`.

### `class MediaPauser`
- State: `threading.Lock`, `_paused: bool`, `_paused_ids: list[str]` (the
  `SourceAppUserModelId` of each session we paused).
- `is_paused() -> bool` - lock-guarded read.
- `pause() -> None`:
  1. If already `_paused`, return (idempotent, like `duck`).
  2. Get the SMTC session manager
     (`GlobalSystemMediaTransportControlsSessionManager.request_async()`,
     awaited to completion - see async note).
  3. For each session, read `GetPlaybackInfo().PlaybackStatus`; if it equals
     `PLAYING`, call `TryPauseAsync()` (awaited) and record its
     `SourceAppUserModelId`.
  4. Persist the recorded ids to `pause_state.json` (crash safety).
  5. Set `_paused = True`.
- `resume() -> None`:
  1. For each recorded id, re-find the live session and call `TryPlayAsync()`
     (awaited). A session that no longer exists is skipped.
  2. Clear `_paused_ids`, clear `pause_state.json`, set `_paused = False`.
- Every step is wrapped so no `winrt`/COM error propagates.

### `class NullPauser`
No-op `is_paused()/pause()/resume()`. Used on non-Windows, when `winrt` is
missing, or as the daemon default until the real backend is injected. Mirrors
`NullDucker`.

### Crash safety
- `pause_state.json` under `SONARA_DIR` holds `{"apps": [<appId>, ...]}`.
- `resume_from_state_file() -> None`: on daemon startup, read the file, re-find
  any live SMTC session whose `SourceAppUserModelId` matches a recorded id, call
  `TryPlayAsync()`, then delete the file. Best-effort, never raises. Called at
  daemon startup right next to the existing `restore_from_state_file()` for
  ducking.

### WinRT async note
The SMTC calls (`request_async`, `TryPauseAsync`, `TryPlayAsync`) return
`IAsyncOperation`s. They are awaited to completion via a small blocking helper
that runs the awaitable on an event loop / dedicated thread, following the same
lazy-`winrt` pattern the tts module already uses for `SpeechSynthesizer`. The
helper is bounded and best-effort; a hang or error resolves to "did nothing".
Exact mechanism is an implementation detail for the plan, not a design decision.

---

## 3. Daemon Routing

### Injection
- `platform/windows/__init__.py` injects `pauser=MediaPauser()` alongside the
  existing `ducker=AudioDucker()`.
- `platform/base.py` gains a `pauser` attribute (duck-typed:
  `pause`/`resume`/`is_paused`), defaulting to `None`.
- `SpeechDaemon.__init__` accepts `pauser=None`; when `None`, defaults to
  `NullPauser()` (mirrors the existing `ducker` default).

### Engage / restore (replace `_maybe_duck` / `_maybe_restore`)
- `_maybe_engage_audio() -> None` (the `on_play` callback passed to
  `speaker.speak`):
  - `duck` mode and not ducked -> `ducker.duck(self._duck_exclude_pids(), self._duck_level())`
  - `pause` mode and not paused -> `pauser.pause()`
  - `off` -> nothing
- `_maybe_restore_audio() -> None` (global idle / stop / app-pause / mode change):
  defensively disengage BOTH backends -
  `if ducker.is_ducked(): ducker.restore()` and
  `if pauser.is_paused(): pauser.resume()`.
- The `on_play` wiring in `_speak_loop_once` passes `_maybe_engage_audio`, and
  keeps the #90 rule: `on_play = None` for `session_change` items (they never
  engage any backend).
- Every current call site of `_maybe_restore()` (idle branch, paused branch,
  stop handler) calls `_maybe_restore_audio()` instead.

### Handlers
- `SET_AUDIO_MODE` handler:
  1. Validate `mode` in `{"off","duck","pause"}`; ignore unknown values.
  2. Set `self.config["audio_mode"] = mode`; `save_config`.
  3. Disengage the previously-active backend immediately: call
     `_maybe_restore_audio()` (so switching duck->pause while idle-ducked does
     not leave other apps ducked, and switching to off restores now - matching
     today's "un-duck immediately on turn-off").
  4. Speak a cue via `_speak_cue(..., exempt_mute=True, pause_exempt=True)`:
     `off` -> "Audio off.", `duck` -> "Audio ducking.", `pause` -> "Media pause."
     (final wording adjustable during implementation; keep short).
  5. `self._wake.set()`.
- `SET_AUDIO_CONTROL` handler: becomes the shim - translate
  `enabled` to `mode` (`True->"duck"`, `False->"off"`) and run the exact same
  body as `SET_AUDIO_MODE` (extract a shared internal `_apply_audio_mode(mode)`).
- `SET_DUCK_LEVEL` handler: unchanged except the re-apply guard keys off
  `_audio_duck_on()` instead of `_audio_control_on()`.

### Mode-switch-while-speaking
Consistent with today's ducking: switching modes disengages the old backend
immediately; the new backend engages at the NEXT playback (not retroactively
mid-utterance). No special-casing.

---

## 4. Settings UI (`settings.html` + `webui.py`)

### Markup
Replace the "Audio duck" on/off switch (the `#duck-switch` pref block) with a
segmented three-way control, markup identical in shape to `#summary-seg`:

```html
<div class="segments" id="audio-seg" role="tablist">
  <button data-mode="off">Off</button>
  <button data-mode="duck">Duck</button>
  <button data-mode="pause">Pause</button>
</div>
<div class="hint" id="audio-mode-hint">...</div>
```

Hints:
- `off`: "Other apps play at full volume."
- `duck`: "Lower other apps' volume while speaking."
- `pause`: "Pause music and video while speaking."

Keep the **Duck level** row (`#duck-row`) unchanged in markup.

### JS
- Render: active segment = `s.config.audio_mode` (default `"off"`); set the
  `.active` class on the matching button; set `#audio-mode-hint` text.
- Gate the duck-level row via the existing helper:
  `gateRow("duck-row", s.config.audio_mode !== "duck", ["duck"])`
  (active only in duck mode; grayed in off and pause).
- Click handler on `#audio-seg button`: `set("audio_mode", b.dataset.mode)`.
- Remove the old `#duck-switch` click handler.

### webui plumbing
- Add `audio_mode` to `_PAGE_KEYS`.
- Add to `_MSG_KEYS`:
  `"audio_mode": lambda v: {"type": "set_audio_mode", "mode": str(v)}`.
- Keep `audio_control` and `duck_level` entries for compat.

---

## 5. Testing

### `tests/test_pausing.py` (new, mock SMTC)
Inject a fake session manager / sessions (patch the module's session-manager
accessor, mirroring how `test_ducking.py` patches `_all_sessions`):
- pauses only sessions whose status is *Playing*; leaves *Paused*/*Stopped*
  alone.
- records exactly the paused sessions; `resume()` plays back only those.
- a session that vanished between pause and resume is skipped, others still
  resume.
- never raises when the session manager / a `Try*Async` call throws.
- `pause()` writes `pause_state.json`; `resume()` clears it.
- `resume_from_state_file()` re-plays recorded ids and deletes the file; never
  raises on failure.
- `NullPauser` is a no-op.

### Daemon tests (extend the duck patterns; `FakePauser` in `daemon_helpers`)
- `audio_mode="off"` -> neither `ducker.duck` nor `pauser.pause` called at
  playback.
- `audio_mode="duck"` -> `ducker.duck` called at playback (existing behavior
  preserved), `pauser` untouched.
- `audio_mode="pause"` -> `pauser.pause` called at playback, `ducker` untouched.
- both backends disengaged at global idle (`_maybe_restore_audio`).
- session-change announcement engages neither backend (extends #90).
- `SET_AUDIO_MODE`: persists `audio_mode`, speaks the cue, and disengages the
  previously-active backend (duck->pause while ducked -> `ducker.restore`
  called; ->off restores now).
- `SET_AUDIO_CONTROL` shim: `enabled=True` -> `audio_mode="duck"`,
  `enabled=False` -> `audio_mode="off"`.
- `SET_DUCK_LEVEL` re-applies only when `_audio_duck_on()`.
- migration: config with `audio_control=True` and no `audio_mode` loads as
  `audio_mode="duck"`; `audio_control=False`/absent -> `"off"`.

### webui / config tests
- `audio_mode` round-trips through the page -> `SET_AUDIO_MODE`.
- duck-level row gated to duck mode (assert the gate input list / disabled state
  matches, following existing gate tests).
- `DEFAULTS["audio_mode"] == "off"`.

### `FakePauser` (in `tests/daemon_helpers.py`)
`pause()/resume()/is_paused()` with `pause_calls`/`resume_calls` counters,
mirroring `FakeDucker`. `make_daemon` injects it alongside `FakeDucker`.

---

## Non-Goals (YAGNI)
- No per-app pause allow/deny list.
- No duck-as-fallback for non-media audio in pause mode (pure pause).
- No cross-fade or gradual volume ramps.
- No macOS/Linux pause backend (`NullPauser` there, as ducking is Windows-only).
- No retroactive mid-utterance engage on mode switch.

---

## File Map
- `src/sonara/config.py` - `audio_mode` default + migration.
- `src/sonara/protocol.py` - `SET_AUDIO_MODE`.
- `src/sonara/platform/windows/pausing.py` - **new** `MediaPauser`, `NullPauser`,
  `resume_from_state_file`.
- `src/sonara/platform/windows/__init__.py` - inject `pauser`.
- `src/sonara/platform/base.py` - `pauser` attribute.
- `src/sonara/daemon.py` - helpers, `_maybe_engage_audio`/`_maybe_restore_audio`,
  `_apply_audio_mode`, `SET_AUDIO_MODE` handler + `SET_AUDIO_CONTROL` shim,
  startup `resume_from_state_file()`.
- `src/sonara/cli.py` - `audio-mode` subcommand.
- `src/sonara/webui.py` - `audio_mode` in `_PAGE_KEYS`/`_MSG_KEYS`.
- `src/sonara/settings.html` - segmented control + gating + JS.
- `tests/test_pausing.py`, `tests/daemon_helpers.py`, and extensions to the
  daemon/webui/config test modules.
