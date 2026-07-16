# Defer Session-Change Alert to Synthesis-Ready - Design

**Status:** Approved for planning (2026-07-16). Issue #94.

**Goal:** Stop the session-change alert (chime + spoken "reading from X") from
playing seconds before the actual content on slow engines. Defer it so it plays
at the content utterance's synthesis-ready moment (`on_play`), right before the
first content sample.

---

## Problem

On a cross-session handoff the router emits a standalone `session_change`
`SpeechItem`. The daemon speaks it immediately: it fires the chime earcon and
speaks the announcement through the fast cue voice (warm Kokoro, ~0.3s)
(`daemon.py:2189-2211`). The *next* loop iteration speaks the content through
the configured voice. With Chatterbox the content synthesizes for ~5s before any
audio, so the user hears: chime + alert, then ~5s of silence, then content. The
alert is not early by accident - it is a separate, faster utterance that
finishes long before the slow one starts.

## Fix (overview)

Defer the alert to the content utterance's `on_play` callback, which every
engine fires right after synthesis and before playback (`tts.py`: Chatterbox
fires it in `_ChatterboxHandle` when the first chunk is ready; Kokoro and native
fire it in `run()` after `wav_bytes`/`_synthesize_wav`, before
`_play_wav_bytes`). So the chime + alert play the instant content synthesis
finishes, contiguous with the content audio.

Confirmed load-bearing facts:
- `on_play` fires for ALL engines (verified in `run()` / `_ChatterboxHandle`), so
  one uniform mechanism works with no per-engine branching.
- `on_play` fires AFTER the content's synthesis completes, so synthesizing the
  alert inside it never overlaps the content synthesis (no concurrent-engine
  re-entrancy).

## Design

### Scope
- **All engines**, uniformly.
- **Only when `fast_cues` is on** (the default). With `fast_cues` off the user has
  opted every cue into the content voice; deferring a slow content-voice alert
  into `on_play` would just add latency, so that path keeps the legacy immediate
  announcement unchanged.
- **Router untouched.** The deferral is entirely daemon-side.

### Mechanism (daemon-side)

New daemon state: `self._pending_preamble = None`, holding `(session, text)` or
`None`.

In `_speak_loop_once`, replace the current session_change handling:

1. **Muted/dropped item:** if an item is dropped by mute, and a pending preamble
   is stashed for that item's session, clear it (mute silences handoffs, so the
   deferred alert is dropped too - matching today, where a muted handoff plays
   nothing).

2. **`session_change` item:**
   - If `fast_cues` is on: stash `self._pending_preamble = (item.session, item.text)`
     and RETURN without speaking or firing the chime. (The alert now rides on the
     next content utterance.)
   - If `fast_cues` is off: legacy path unchanged - fire the chime and speak the
     item immediately with `on_play=None` and no cue-voice override (content voice).

3. **Content item (kind != session_change):**
   - Determine the preamble: if `self._pending_preamble` is set AND its session ==
     `item.session`, take its text; otherwise (no preamble, or a stale
     session mismatch) take none. Clear `self._pending_preamble` either way.
   - If a preamble applies, wrap `on_play` so that at synthesis-ready it: (a) fires
     the `session_change` chime, (b) speaks the alert text via the fast cue voice
     through a NON-tracked cue-speak, (c) then runs the normal
     `_maybe_engage_audio` (duck/pause). Each step is individually guarded so a
     failure never breaks or blocks the content utterance.
   - If no preamble applies, `on_play = self._maybe_engage_audio` (unchanged
     content behavior).

The `session_change` synthetic item (from the router, `id=0`) is no longer passed
to `speak`/`note_spoken` on the deferred path; it is consumed by the stash. This
is safe because it is synthetic (not a channel item) and carries no heard/voiced
accounting. (The plan verifies no accounting depends on it.)

### New Speaker method: non-tracked cue speak

`Speaker.speak_cue_untracked(text, voice, rate=None) -> None`

Synthesizes and plays *text* through *voice*, blocking until done (bounded by the
existing `_wait_timeout`), WITHOUT registering the proc as `self._current`. This
is the crux of safety: the content utterance's `speak()` tracks its own proc in
`self._current` for cancellation; a re-entrant `self.speaker.speak(alert)` from
inside the content's `on_play` would overwrite `self._current` and break
cancellation of the content mid-playback. `speak_cue_untracked` deliberately does
not touch `self._current`, so content cancellation stays intact. The alert itself
is not separately cancellable (a ~1s cue) - acceptable.

Implementation shape:
```python
def speak_cue_untracked(self, text, voice, rate=None) -> None:
    if self._say_runner is None:
        return
    r = self._rate if rate is None else rate
    try:
        proc = self._say_runner(text, voice, r)   # no on_play
        try:
            proc.wait(timeout=self._wait_timeout)
        except subprocess.TimeoutExpired:
            proc.terminate()
    except Exception:  # noqa: BLE001 - a cue must never break the content utterance
        pass
```

### Ordering guarantee

For every engine the resulting audio order is: [content synthesis, silent] ->
chime -> alert -> content audio. For Chatterbox `on_play` fires inside
`handle.wait()` (after `self._current` is already the content handle, so
cancellation is preserved); for Kokoro/native `on_play` fires inside `run()`
before playback and before `self._current` is assigned, so there is nothing to
clobber. `speak_cue_untracked` touching no shared cancellation state makes both
cases safe.

## Testing

Daemon tests (new `tests/test_daemon_alert_timing.py`), using `FakeSpeaker`
(whose `speak` already fires `on_play` synchronously):
- Handoff, `fast_cues` on: the `session_change` item is stashed, not spoken, and no
  chime fires on that iteration; the pending preamble is recorded.
- The following content item: is spoken, and during its `on_play` the chime fires
  AND `speak_cue_untracked` is called with the alert text and the cue voice AND
  audio is engaged - in that order.
- `fast_cues` off: the `session_change` item is spoken immediately (legacy), chime
  fires on its own iteration, no preamble stashed.
- A muted content item clears a pending preamble for its session (no alert
  leaks onto a later unrelated utterance).
- A pending preamble whose session does not match the next content item is
  dropped (defensive), not applied.
- The content utterance's `on_play` still engages audio (duck/pause) after the
  alert.

`FakeSpeaker` gains a `speak_cue_untracked(text, voice, rate=None)` that records
`(text, voice)` into a list (e.g. `cue_untracked_calls`).

## Non-Goals (YAGNI)
- No change to the router or the multi-session ordering/announcement logic.
- No deferral when `fast_cues` is off.
- No cross-fade or timing tuning; the alert plays immediately at synthesis-ready.
- No making the alert cancellable.

## File Map
- `src/sonara/speaker.py` - add `speak_cue_untracked`.
- `src/sonara/daemon.py` - `_pending_preamble` state; rework the session_change /
  content handling in `_speak_loop_once`; preamble `on_play` wrapper.
- `tests/daemon_helpers.py` - `FakeSpeaker.speak_cue_untracked` recorder.
- `tests/test_daemon_alert_timing.py` - new tests.
