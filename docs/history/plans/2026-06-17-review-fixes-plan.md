# Review Fixes Implementation Plan

> **For agentic workers:** TDD, one fix per commit. Steps use `- [ ]`.

**Goal:** Land the confirmed review fixes (H1, H2, M2, M4, M6, M7, M8, M9, M10,
L2, L3) without changing the protocol wire format.

**Architecture:** Most fixes are localized in `daemon.py` (voice ownership,
cancel-epoch, decision claim, nav owner, pause race), with seam additions in
`speaker.py`, `platform/base.py`, `platform/windows/hotkeys.py`,
`platform/macos/hotkeys.py`, and robustness in `platform/windows/supervisor.py`
and `bin/sonari-hook.cmd`.

**Tech Stack:** Python 3.9+, pytest, threading.

---

### Task 1 (H1): Hold voice ownership during an open prose message
- Modify `src/sonari/daemon.py`: add `self._open_msg`; mark on non-final PROSE;
  clear on PROSE final, FLUSH, SESSION_END, and on the `turn_done` earcon
  (carrying `session`). Gate both `_voice_owner` release sites on
  `owner not in _open_msg`.
- Modify `src/sonari/hooks_entry.py`: Stop → `turn_done` earcon includes `session`.
- Test `tests/test_daemon_prose.py`: two-session strand - owner keeps speaking
  across an inter-chunk drain when another session flips foreground.

### Task 2 (M2): Capture cancel epoch at claim time
- Modify `src/sonari/speaker.py`: add `cancel_epoch()`; `speak(text, cancel_epoch=None)`.
- Modify `src/sonari/daemon.py`: capture epoch under `_lock` at claim; pass to speak().
- Test `tests/test_speaker.py`: a cancel between epoch-capture and synth marks the
  utterance not-completed.

### Task 3 (L2): Lock-guard the pause re-queue
- Modify `src/sonari/daemon.py` `_speak_loop_once`: re-check `_paused.is_set()`
  inside `_lock`; else `note_spoken`.
- Test `tests/test_daemon_pause_mute.py`: FLUSH racing a paused utterance does not
  resurrect it.

### Task 4 (M4 + L3): Decisions + nav claim the voice for the foreground
- Modify `src/sonari/daemon.py`: `_claim_for_decision(session)`; use in
  CHOICE/PLAN/PERMISSION. `_nav` sets `_voice_owner` + clears captured.
- Test `tests/test_daemon_decisions.py`: decision enqueued for foreground while a
  background session owns the voice. `tests/test_daemon_nav.py`: nav sets owner.

### Task 5 (M6): JUMP_DECISION drops pending + marks current heard
- Modify `src/sonari/queue.py`: `jump_to_decision()` returns dropped list.
- Modify `src/sonari/daemon.py`: `_drop_pending` + mark current heard.
- Test `tests/test_queue.py` + `tests/test_daemon_decisions.py`.

### Task 6 (H2 + M7): Hotkey reload seam
- Modify `src/sonari/platform/base.py`: `reload(dispatch)` default = stop+start.
- Modify `src/sonari/platform/windows/hotkeys.py`: `stop()` joins the thread.
- Modify `src/sonari/platform/macos/hotkeys.py`: override `reload` → write_resolved
  + launchctl kickstart.
- Modify `src/sonari/daemon.py` `_reload_hotkeys`: call the seam, honor kill switch.
- Modify `commands/keymap.md`: accurate "applied live" wording.
- Tests `tests/test_win_hotkeys.py` (stop joins before start re-registers),
  `tests/test_macos_hotkeys.py` (reload writes resolved + kickstarts).

### Task 7 (M8): Release the conn permit on spawn failure
- Modify `src/sonari/daemon.py` `_spawn_conn_handler`: try/except start; release.
- Test `tests/test_daemon_conn.py`.

### Task 8 (M9): Harden settings.json parsing
- Modify `src/sonari/platform/windows/supervisor.py`: isinstance guards.
- Test `tests/test_win_settings_hooks.py`: malformed shapes return False.

### Task 9 (M10): Robust sonari-hook.cmd
- Modify `bin/sonari-hook.cmd`: pythonw → pyw -3 fallback; log to ~/.sonari/hook.log.
- Test `tests/test_bin_shims.py` (content assertions).

### Task 10: Full suite + review
- Run platform-independent suite; expect prior green + new tests, known env
  failure unchanged. Dispatch a review workflow over the diff.
