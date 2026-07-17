# Defer Session-Change Alert to Synthesis-Ready Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Defer the session-change alert (chime + spoken "reading from X") so it plays at the content utterance's `on_play` (synthesis-ready) moment instead of seconds earlier on slow engines.

**Architecture:** Daemon-side deferral (router untouched). When `fast_cues` is on, the daemon stashes the session-change item instead of speaking it, and replays the chime + alert from the content utterance's `on_play` callback via a new non-tracked cue-speak that never clobbers the content utterance's cancellation tracking. When `fast_cues` is off, the legacy immediate announcement is unchanged.

**Tech Stack:** Python 3.14 stdlib. Existing Speaker / say_runner / `on_play` machinery.

## Global Constraints

- Python 3.14, stdlib only. No em-dashes anywhere (code, comments) - use en-dashes, commas, or rephrase.
- Applies to all engines, but only when `fast_cues` is on (default True). `fast_cues` off keeps the legacy immediate content-voice announcement.
- The alert cue must never break, block indefinitely, or clobber the content utterance: `speak_cue_untracked` must not touch `self._current`, must swallow errors, and must bound its wait by the existing `_wait_timeout`.
- Router and multi-session ordering logic are out of scope (not modified).

---

### Task 1: Speaker.speak_cue_untracked

**Files:**
- Modify: `src/sonara/speaker.py` (add a method after `speak()`, which ends at line 96)
- Modify: `tests/daemon_helpers.py` (add `FakeSpeaker.speak_cue_untracked` recorder)
- Test: `tests/test_speaker.py`

**Interfaces:**
- Produces: `Speaker.speak_cue_untracked(text, voice, rate=None) -> None` - synthesizes and plays *text* through *voice*, blocking until done (bounded by `self._wait_timeout`), WITHOUT registering the proc as `self._current`.
- Produces: `FakeSpeaker.speak_cue_untracked(text, voice, rate=None)` recording `(text, voice)` into `self.cue_untracked_calls`.

- [ ] **Step 1: Write the failing test**

Add to `tests/test_speaker.py`:

```python
def test_speak_cue_untracked_runs_without_touching_current():
    from sonara.speaker import Speaker
    calls = []

    class _Proc:
        returncode = 0
        def wait(self, timeout=None):
            calls.append(("wait", timeout)); return 0
        def terminate(self):
            calls.append("terminate")

    def runner(text, voice, rate):
        calls.append(("run", text, voice, rate)); return _Proc()

    sp = Speaker(voice="content-voice", rate=200, say_runner=runner)
    sp._current = "CONTENT_PROC"                 # simulate a tracked content proc
    sp.speak_cue_untracked("Reading from repo.", "af_heart")
    assert ("run", "Reading from repo.", "af_heart", 200) in calls
    assert any(c[0] == "wait" for c in calls if isinstance(c, tuple))
    assert sp._current == "CONTENT_PROC"          # never touched -> content cancel intact


def test_speak_cue_untracked_uses_explicit_rate_and_survives_errors():
    from sonara.speaker import Speaker

    def boom(text, voice, rate):
        raise RuntimeError("synth down")

    sp = Speaker(voice="v", rate=200, say_runner=boom)
    sp.speak_cue_untracked("hi", "af_heart", rate=150)   # must not raise
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_speaker.py -k cue_untracked -v`
Expected: FAIL (`AttributeError: 'Speaker' object has no attribute 'speak_cue_untracked'`).

- [ ] **Step 3: Implement the method**

In `src/sonara/speaker.py`, add directly after the `speak()` method (after line 96, before `cancel()`):

```python
    def speak_cue_untracked(self, text: str, voice, rate=None) -> None:
        """Synthesize and play *text* through *voice*, blocking until done, WITHOUT
        registering the proc as self._current. Used to replay a session-change
        alert from inside the content utterance's on_play (#94): a re-entrant
        speak() would overwrite self._current and break the content utterance's
        cancellation. The cue is short and not separately cancellable; a failure
        must never break the content utterance."""
        if self._say_runner is None:
            return
        r = self._rate if rate is None else rate
        try:
            proc = self._say_runner(text, voice, r)
            try:
                proc.wait(timeout=self._wait_timeout)
            except subprocess.TimeoutExpired:
                proc.terminate()
        except Exception:  # noqa: BLE001 - a cue must never break the content utterance
            pass
```

- [ ] **Step 4: Run to verify pass**

Run: `python -m pytest tests/test_speaker.py -k cue_untracked -v`
Expected: PASS (2 tests).

- [ ] **Step 5: Add the FakeSpeaker recorder**

In `tests/daemon_helpers.py`, in `FakeSpeaker.__init__`, add after `self.speak_voices = ...`:

```python
        self.cue_untracked_calls = []  # (text, voice) from speak_cue_untracked (#94)
```

and add a method to `FakeSpeaker` (next to `speak`):

```python
    def speak_cue_untracked(self, text: str, voice, rate=None) -> None:
        self.cue_untracked_calls.append((text, voice))
```

- [ ] **Step 6: Run the speaker + daemon-helper-dependent suites**

Run: `python -m pytest tests/test_speaker.py -q`
Expected: PASS (no regressions).

- [ ] **Step 7: Commit**

```bash
git add src/sonara/speaker.py tests/daemon_helpers.py tests/test_speaker.py
git commit -m "feat(speaker): speak_cue_untracked for replaying a cue without clobbering _current (#94)"
```

---

### Task 2: Daemon defers the session-change alert

**Files:**
- Modify: `src/sonara/daemon.py` (add `self._pending_preamble = None` near line 154; rework the session_change / content block at lines 2187-2216)
- Test: `tests/test_daemon_alert_timing.py` (new)

**Interfaces:**
- Consumes: `Speaker.speak_cue_untracked` and `FakeSpeaker.cue_untracked_calls` (Task 1); `self._maybe_engage_audio`, `self._cue_voice`, `self._earcon` (existing).
- Produces: `self._pending_preamble` (a `(session, text)` tuple or `None`) and the deferred-alert behavior.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_daemon_alert_timing.py`:

```python
"""#94: the session-change alert plays at the content's synthesis-ready moment
(on_play), not seconds earlier. Deferral is daemon-side and only when fast_cues
is on; fast_cues off keeps the legacy immediate announcement."""
from sonara.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _seed_content(daemon, session, text="The digest body."):
    ch = daemon.router.channel(session)
    ch.append(SpeechItem(id=1, session=session, kind="prose", text=text,
                         is_decision=False))
    ch.turn_done = True
    daemon.router._replay_authorized.add(session)


def _handoff(foreground="a", target="b"):
    daemon, queue, speaker, sessions, config = make_daemon(foreground=foreground)
    daemon.router._last_active = foreground          # so switching is a real handoff
    _seed_content(daemon, target)
    return daemon, speaker, config


def test_fast_cues_on_defers_alert_to_content_on_play():
    daemon, speaker, config = _handoff()
    config["fast_cues"] = True

    daemon._speak_loop_once()                        # session_change item -> stashed
    assert daemon._pending_preamble is not None
    assert daemon._pending_preamble[0] == "b"
    assert speaker.spoken == []                      # nothing spoken yet
    assert speaker.earcons == []                     # chime NOT fired yet
    assert speaker.cue_untracked_calls == []

    daemon._speak_loop_once()                        # content -> on_play fires the alert
    assert "The digest body." in speaker.spoken
    assert speaker.earcons == ["session_change"]     # chime at synthesis-ready
    assert len(speaker.cue_untracked_calls) == 1     # alert spoken via cue voice
    assert speaker.cue_untracked_calls[0][1] == daemon._cue_voice()
    assert daemon._pending_preamble is None


def test_fast_cues_off_speaks_alert_immediately():
    daemon, speaker, config = _handoff()
    config["fast_cues"] = False

    daemon._speak_loop_once()                        # session_change spoken now (legacy)
    assert speaker.earcons == ["session_change"]
    assert speaker.spoken                            # announcement spoken immediately
    assert daemon._pending_preamble is None
    assert speaker.cue_untracked_calls == []         # no deferred cue path


def test_muted_content_drops_pending_alert():
    daemon, speaker, config = _handoff()
    config["fast_cues"] = True
    daemon._speak_loop_once()                        # stash preamble for "b"
    assert daemon._pending_preamble is not None
    daemon._mute_level = 1                            # mute drops the content...
    daemon._speak_loop_once()                        # ...and the deferred alert with it
    assert daemon._pending_preamble is None
    assert speaker.cue_untracked_calls == []
    assert speaker.earcons == []                     # no chime for a muted handoff


def test_stale_preamble_for_other_session_is_dropped():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    config["fast_cues"] = True
    daemon._pending_preamble = ("b", "Reading from b.")   # stale, for a different session
    _seed_content(daemon, "a", text="Foreground content.")
    daemon._speak_loop_once()                        # content for "a"
    assert "Foreground content." in speaker.spoken
    assert speaker.cue_untracked_calls == []         # stale alert not applied
    assert daemon._pending_preamble is None


def test_preamble_on_play_still_engages_audio():
    daemon, speaker, config = _handoff()
    config["fast_cues"] = True
    config["audio_mode"] = "pause"
    daemon._speak_loop_once()                        # stash
    daemon._speak_loop_once()                        # content on_play: alert THEN engage
    assert speaker.cue_untracked_calls              # alert played
    assert daemon.pauser.pause_calls == 1            # audio still engaged after the alert
```

- [ ] **Step 2: Run to verify failure**

Run: `python -m pytest tests/test_daemon_alert_timing.py -v`
Expected: FAIL (`AttributeError: _pending_preamble`, and the alert is spoken immediately, not deferred).

- [ ] **Step 3: Add the state field**

In `src/sonara/daemon.py`, directly after `self._current_item = None` (line 154) add:

```python
        self._pending_preamble = None             # (session, alert_text) deferred to content on_play (#94)
```

- [ ] **Step 4: Rework the session_change / content handling**

In `src/sonara/daemon.py`, replace the block at lines 2187-2216 (from `if muted:` through the final `note_spoken`) with:

```python
        if muted:
            # A dropped item also drops a pending alert for its session: mute
            # silences handoffs, so the deferred chime + announcement go too.
            if (self._pending_preamble is not None
                    and self._pending_preamble[0] == item.session):
                self._pending_preamble = None
            return
        if item.kind == "session_change":
            if self.config.get("fast_cues", True):
                # Defer the alert (#94): stash it and play the chime + spoken
                # announcement from the CONTENT utterance's on_play, so a slow
                # engine no longer plays the alert seconds before the audio.
                self._pending_preamble = (item.session, item.text)
                self._current_item = None
                return
            # fast_cues off: legacy immediate announcement in the content voice.
            try:
                self._earcon("session_change")
            except Exception:  # noqa: BLE001
                pass
            try:
                completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch,
                                               on_play=None)
            except Exception:  # noqa: BLE001
                self._signal_speak_failure()
                completed = False
            if not self._requeue_or_note(item, completed):
                self.note_spoken(item, completed)
            return
        # Content item. A stashed alert for THIS session plays as a preamble at
        # synthesis-ready (on_play): chime, then the spoken alert via the fast cue
        # voice (non-tracked so it never clobbers this utterance's cancellation),
        # then the normal duck/pause engage. #90's "announcement never ducks" is
        # preserved: the alert cue itself is played WITHOUT on_play, and the duck/
        # pause engage happens for the CONTENT, after the alert.
        preamble = None
        if (self._pending_preamble is not None
                and self._pending_preamble[0] == item.session):
            preamble = self._pending_preamble[1]
        self._pending_preamble = None
        if preamble is not None:
            cue_voice = self._cue_voice()
            rate = self.config.get("rate", 200)

            def on_play(_text=preamble, _voice=cue_voice, _rate=rate):
                try:
                    self._earcon("session_change")
                except Exception:  # noqa: BLE001
                    pass
                try:
                    self.speaker.speak_cue_untracked(_text, _voice, _rate)
                except Exception:  # noqa: BLE001
                    pass
                self._maybe_engage_audio()
        else:
            on_play = self._maybe_engage_audio
        try:
            completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch,
                                           on_play=on_play,
                                           **self._cue_voice_override(item))
        except Exception:  # noqa: BLE001
            self._signal_speak_failure()
            completed = False
        if not self._requeue_or_note(item, completed):
            self.note_spoken(item, completed)
```

- [ ] **Step 5: Run to verify pass**

Run: `python -m pytest tests/test_daemon_alert_timing.py -v`
Expected: PASS (5 tests).

- [ ] **Step 6: Run the regression set (session-change + duck + question flow)**

Run: `python -m pytest tests/test_daemon_duck_announcement.py tests/test_daemon_duck_timing.py tests/test_daemon_summary_mode.py tests/test_daemon_question_flow.py tests/test_daemon_digest_ordering.py -q`
Expected: PASS. If a test asserted the old "session_change spoken immediately" behavior with `fast_cues` on and now fails, it is exercising the behavior this task intentionally changed; update it minimally to the deferred model (assert the alert lands on the content's on_play) without weakening it, and note it in the report. If it relied on `fast_cues` being on by default and a chime firing on the session_change iteration, that is the changed behavior.

- [ ] **Step 7: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_alert_timing.py
git commit -m "fix(daemon): defer session-change alert to the content's synthesis-ready moment (#94)"
```

---

## Final verification

- [ ] Run the full suite: `python -m pytest -q`
  Expected: all pass except the documented pre-existing baseline failures (test_bin_sonara.py WinError 193 x3, test_daemon_ducking `duck_level==20`, test_paths x2, test_transport, test_win_tts x2 + 1 error). No new failures.
- [ ] Deploy and verify live with Chatterbox: two active sessions, one finishing a turn while the other is foreground. Expected: silence during Chatterbox synthesis, then chime + "reading from X" + content contiguous, instead of the alert seconds early.

---

## Self-Review

**Spec coverage:** `speak_cue_untracked` (non-tracked cue) -> Task 1. `_pending_preamble` state + stash-on-session_change + preamble on_play + fast_cues-off legacy + mute-drop + session-match drop + engage-after-alert -> Task 2. All spec sections covered. Router untouched (non-goal respected).

**Placeholder scan:** No TBD/TODO. Every code step is complete. Task 2 Step 6 gives concrete instructions for the one judgment area (a pre-existing test that asserted the old immediate-announcement behavior), not a vague "fix tests".

**Type consistency:** `speak_cue_untracked(text, voice, rate=None)` signature identical in the real method (Task 1), the FakeSpeaker recorder (Task 1), and the daemon call site (Task 2). `_pending_preamble` is a `(session, text)` tuple everywhere it is read/written. `cue_untracked_calls` records `(text, voice)`, matching the Task 2 assertions (`[0][1] == cue_voice`).
