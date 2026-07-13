# Question Lead-In Settle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A question's lead-in context is always read before the question, even when the CHOICE signal beats its prose to the daemon.

**Architecture:** Extend the #14 settle window to the CHOICE path. On a question (summary mode), build the question item immediately (chime already fired; `_await_choice`/`_options`/history set now), stash the item in `_pending_decision`, and arm the settle window instead of gathering the lead-in. `_settle_fire` runs the decision flow (`_maybe_summarize` + `_enqueue_or_hold_decision`) when `_pending_decision` has the session, else the plain turn-end digest.

**Tech Stack:** Python 3.14 stdlib, pytest.

## Global Constraints

- Reuse #14's settle machinery (`_arm_settle`/`_settle_fire`/`_settle_pending`/`_cancel_settle`); do not add a second timer system.
- A question blocks the turn (no `turn_done`), so the decision settle and the turn-end settle never contend for one turn.
- `_await_choice` must be set at CHOICE arrival (immediate) so the paired permission notification is still suppressed.
- Non-summary mode keeps the original immediate CHOICE path.
- Scope: CHOICE (AskUserQuestion) only. No em-dashes.

---

### Task 1: Defer the question's content through the settle window

**Files:**
- Modify: `src/sonara/daemon.py` — new state `_pending_decision` in `__init__`; the CHOICE handler; `_settle_fire`; the FLUSH and SESSION_END handlers.
- Test: `tests/test_daemon_summary_mode.py` — update the `_choice` helper and the one inline-CHOICE test to fire the settle; add three tests.

**Interfaces:**
- Consumes: `_arm_settle`/`_settle_fire`/`_settle_gen`/`_settle_pending` (#14), `_maybe_summarize`, `_enqueue_or_hold_decision`, `_await_choice`, `_options`, `_choice_text`/`_choice_notes`/`_selection_cue`, `SpeechItem`.
- Produces: new state `_pending_decision: dict` (session -> question `SpeechItem`); `_settle_fire` branches on it.

- [ ] **Step 1: Update the `_choice` helper + inline test, add new tests (failing)**

In `tests/test_daemon_summary_mode.py`, update the `_choice` helper (~line 132) to fire the settle deterministically:

```python
def _choice(daemon, session="fg"):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE, "session": session,
                           "questions": [{"question": "Pick one?", "options": ["a", "b"]}]})
    _fire_settle(daemon, session)
```

Update `test_decisions_still_spoken_with_summary_mode_on` (the one inline-CHOICE test, ~line 53): add `_fire_settle(daemon, "fg")` immediately after its `handle_message({... CHOICE ...})` call, before the assert.

Add these tests at the end of the file:

```python
def test_question_lead_in_after_choice_not_stranded(monkeypatch):
    # The bug: CHOICE arrives before its lead-in prose. With the settle window the
    # late lead-in is digested and the question is held after it (#16).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE, "session": "fg",
                           "questions": [{"question": "Pick one?", "options": ["a", "b"]}]})
    _pad = "This filler sentence carries the lead-in past the threshold. "
    daemon.handle_message(_prose("fg", "The context here. " + _pad * 6, 0, True))  # late lead-in
    daemon._settle_fire("fg", scheduled[-1][1])
    assert len(calls) == 1
    _, _, text = calls[0]
    assert "The context here." in text                     # lead-in digested, not empty
    assert daemon._held_decision.get("fg") is not None      # question held until digest


def test_choice_defers_question_until_settle(monkeypatch):
    # CHOICE alone does not enqueue the question; only the settle fire does (#16).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Short context. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE, "session": "fg",
                           "questions": [{"question": "Pick one?", "options": ["a", "b"]}]})
    ch = daemon.router.channel("fg")
    assert not any(it.is_decision for it in ch.items[ch.cursor:])   # deferred
    daemon._settle_fire("fg", scheduled[-1][1])
    assert any(it.is_decision for it in ch.items[ch.cursor:])       # enqueued after fire


def test_flush_cancels_pending_question_settle(monkeypatch):
    # A new prompt during a question's settle window drops the pending question;
    # a late fire is a no-op (#16, consistent with #13/#14).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE, "session": "fg",
                           "questions": [{"question": "Pick one?", "options": ["a", "b"]}]})
    stale = scheduled[-1][1]
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    daemon._settle_fire("fg", stale)
    ch = daemon.router.channel("fg")
    assert not any(it.is_decision for it in ch.items[ch.cursor:])   # dropped
    assert "fg" not in daemon._pending_decision
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `python -m pytest tests/test_daemon_summary_mode.py -k "question or choice_defers or pending_question" -v`
Expected: FAIL with `AttributeError: _pending_decision`, and the deferral assertions failing (question enqueued immediately today).

- [ ] **Step 3: Add `_pending_decision` state**

In `src/sonara/daemon.py`, next to the settle state (after `_settle_pending`), add:

```python
        self._pending_decision: dict = {}  # session -> question item awaiting its lead-in (#16)
```

- [ ] **Step 4: Defer the CHOICE content in summary mode**

In `src/sonara/daemon.py`, replace the CHOICE handler body:

```python
            digesting = self._maybe_summarize(session)
            text = self._choice_text(msg)
            extras = [e for e in (self._choice_notes(msg),
                                  self._selection_cue(session, verbosity)) if e]
            if extras:
                text = "{0} {1}".format(text, " ".join(extras))
            self._options[session] = text
            entry = self.history.record(session, "choice", text)
            self.history.end_message(session)
            item = SpeechItem(id=self._alloc_id(), session=session, kind="choice",
                              text=text, is_decision=True)
            self._pending_heard[item.id] = entry
            self._await_choice.add(session)
            self._enqueue_or_hold_decision(session, item, digesting)
            return None
```

with:

```python
            text = self._choice_text(msg)
            extras = [e for e in (self._choice_notes(msg),
                                  self._selection_cue(session, verbosity)) if e]
            if extras:
                text = "{0} {1}".format(text, " ".join(extras))
            self._options[session] = text
            entry = self.history.record(session, "choice", text)
            self.history.end_message(session)
            item = SpeechItem(id=self._alloc_id(), session=session, kind="choice",
                              text=text, is_decision=True)
            self._pending_heard[item.id] = entry
            self._await_choice.add(session)
            # The CHOICE can reach the daemon BEFORE its lead-in prose (separate
            # hook processes race), so gathering the lead-in now would find nothing
            # and speak the question alone. Defer through the settle window: gather
            # the lead-in and hold/enqueue the question once the prose lands, so
            # context is heard first (#16). Non-summary speaks prose live, no digest.
            if self.config.get("summary_mode"):
                self._pending_decision[session] = item
                self._arm_settle(session)
            else:
                self._enqueue_or_hold_decision(session, item, False)
            return None
```

- [ ] **Step 5: Branch `_settle_fire` on a pending decision**

In `src/sonara/daemon.py`, replace the tail of `_settle_fire`:

```python
            self._settle_pending.discard(session)
            self._settle_timers.pop(session, None)
            self._maybe_summarize(session)
```

with:

```python
            self._settle_pending.discard(session)
            self._settle_timers.pop(session, None)
            item = self._pending_decision.pop(session, None)
            if item is not None:
                # A question was waiting on its lead-in: gather it now (present
                # after the settle) and hold the question after the context (#16).
                digesting = self._maybe_summarize(session)
                self._enqueue_or_hold_decision(session, item, digesting)
            else:
                self._maybe_summarize(session)
```

- [ ] **Step 6: Cancel the pending decision on FLUSH and SESSION_END**

In `src/sonara/daemon.py`, in the FLUSH handler next to `self._held_decision.pop(session, None)`, add:

```python
            self._pending_decision.pop(session, None)   # drop a question awaiting its lead-in (#16)
```

And in SESSION_END, next to the settle cleanup, add:

```python
            self._pending_decision.pop(session, None)
```

- [ ] **Step 7: Run the summary-mode suite (green)**

Run: `python -m pytest tests/test_daemon_summary_mode.py -q`
Expected: all PASS (new tests + all existing question tests, which now fire the settle via the updated `_choice` helper).

- [ ] **Step 8: Run the whole suite for regressions**

Run: `python -m pytest -q`
Expected: only the 10 known pre-existing environmental failures.

- [ ] **Step 9: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_summary_mode.py
git commit -m "fix(summary): question lead-in read before the question (#16)

The CHOICE message can reach the daemon before its lead-in prose (separate hook
processes race), so the lead-in was summarized empty and the question spoke
alone. Defer the question's content through the #14 settle window: gather the
lead-in and hold the question once the prose settles, so context is heard first.
AskUserQuestion path only.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```
