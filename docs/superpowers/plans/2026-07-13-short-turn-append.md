# Short-Turn Append Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A queued question is never overtaken by a later short turn's content; short turns append like long turns.

**Architecture:** Add an `append` mode to `_replay`. The short-turn digest path in `_maybe_summarize` calls `_replay(..., append=True)` so raw prose is added at the end of the channel (after any queued question), consistent with the long-digest `_enqueue`. Nav/catch-up/repeat keep the default cursor-insert.

**Tech Stack:** Python 3.14 stdlib, pytest.

## Global Constraints

- Only the short-turn digest path changes to append; nav/catch-up/repeat keep cursor-insert.
- No behavior change when the channel queue is empty (append == cursor).
- Preserve `_replay`'s other effects (`_pending_heard`, `turn_done`, announcement suppression, `_replay_authorized`). No em-dashes.

---

### Task 1: `_replay` append mode; short-turn digest appends

**Files:**
- Modify: `src/sonara/daemon.py` — `_replay` signature/body; the short-turn branch of `_maybe_summarize`.
- Test: `tests/test_daemon_summary_mode.py` — add the overtaking test; existing short-turn/lead-in tests must still pass.

**Interfaces:**
- Consumes: `_replay(session, entries)`, `_maybe_summarize` short branch, channel `items`/`cursor`.
- Produces: `_replay(session, entries, append=False)` — `append=True` inserts at end of `ch.items` instead of at `ch.cursor`.

- [ ] **Step 1: Add the overtaking test (failing)**

In `tests/test_daemon_summary_mode.py`, add at the end:

```python
def test_short_answer_does_not_overtake_held_question(monkeypatch):
    # Repro of the live bug (#17): answer a question before it is voiced; the short
    # answer-response must NOT cut ahead of the queued question in the channel.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)           # long lead-in
    _choice(daemon)                                  # settle -> digest dispatched, question held
    daemon._summarize_fn = lambda text, **kw: "The context digest."
    daemon._summary_worker(*calls[0])                # context enqueued + question appended
    ch = daemon.router.channel("fg")
    daemon.handle_message(_prose("fg", "Short answer. ", 0, True))   # short answer-response
    _turn_done(daemon)                               # settle -> short path
    texts = [it.text for it in ch.items]
    q_idx = next(i for i, t in enumerate(texts) if "Pick one?" in t)
    a_idx = next(i for i, t in enumerate(texts) if "Short answer." in t)
    assert q_idx < a_idx                             # question stays ahead of the later short answer
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python -m pytest tests/test_daemon_summary_mode.py::test_short_answer_does_not_overtake_held_question -v`
Expected: FAIL -- today the short answer inserts at the cursor (index 0), ahead of the question, so `a_idx < q_idx`.

- [ ] **Step 3: Add `append` mode to `_replay`**

In `src/sonara/daemon.py`, change the `_replay` signature and insertion point. Replace:

```python
    def _replay(self, session: str, entries) -> None:
```

with:

```python
    def _replay(self, session: str, entries, append: bool = False) -> None:
```

And in the body, replace:

```python
        ch = self.router.channel(session)
        at = ch.cursor
        for e in entries:
            item = SpeechItem(
                id=self._alloc_id(),
                session=session,
                kind=e.kind,
                text=e.text,
                is_decision=e.kind in ("choice", "plan", "permission"),
            )
            self._pending_heard[item.id] = e
            ch.items.insert(at, item)
            at += 1
        if at > ch.cursor:  # only if we actually inserted items
```

with:

```python
        ch = self.router.channel(session)
        # append=True adds at the END (after any queued decision), like the long
        # digest path -- so a new short turn never overtakes a queued question
        # (#17). append=False keeps cursor-insert for explicit user replay
        # (catch_up / nav / repeat), which should read next.
        at = len(ch.items) if append else ch.cursor
        n = 0
        for e in entries:
            item = SpeechItem(
                id=self._alloc_id(),
                session=session,
                kind=e.kind,
                text=e.text,
                is_decision=e.kind in ("choice", "plan", "permission"),
            )
            self._pending_heard[item.id] = e
            ch.items.insert(at, item)
            at += 1
            n += 1
        if n > 0:  # only if we actually inserted items
```

(The docstring's cursor-insert description still holds for the default mode; the append branch is documented inline.)

- [ ] **Step 4: Make the short-turn digest path append**

In `src/sonara/daemon.py`, in `_maybe_summarize`'s short branch, replace:

```python
            if self.sessions.is_foreground(session):
                self._replay(session, entries)
```

with:

```python
            if self.sessions.is_foreground(session):
                # Append (not cursor-insert) so a short turn never overtakes a
                # queued question -- consistent with the long digest path (#17).
                self._replay(session, entries, append=True)
```

- [ ] **Step 5: Run the overtaking test (green)**

Run: `python -m pytest tests/test_daemon_summary_mode.py::test_short_answer_does_not_overtake_held_question -v`
Expected: PASS.

- [ ] **Step 6: Run the full summary-mode suite**

Run: `python -m pytest tests/test_daemon_summary_mode.py -q`
Expected: all PASS (short-turn and short-lead-in ordering tests still hold; append == cursor when the queue is empty).

- [ ] **Step 7: Run the nav/catch-up suite and the whole suite**

Run: `python -m pytest tests/test_daemon_nav_summary.py -q` then `python -m pytest -q`
Expected: nav/catch-up green (default cursor-insert unchanged); whole suite has only the 10 known pre-existing environmental failures.

- [ ] **Step 8: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_summary_mode.py
git commit -m "fix(summary): short turn appends so it can't overtake a queued question (#17)

A held question queued after its context digest was overtaken by a later SHORT
answer-response, because the short-turn path (_replay) inserts at the cursor
while the long-digest path appends. Give _replay an append mode and use it for
the short-turn digest, so new content always appends after a queued decision.
Nav/catch-up/repeat keep cursor-insert. Follow-up to #16.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```
