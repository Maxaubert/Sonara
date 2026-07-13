# User-Only Cancellation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In summary mode, the system never drops a completed message; only a real user action (a new prompt) cancels a session's reading.

**Architecture:** The per-session counter `_summary_gen` in `daemon.py` becomes a *cancel epoch* that advances **only** on a user action (FLUSH / new prompt), never on a turn merely ending. `_maybe_summarize` captures the current epoch at dispatch without advancing it; `_summary_worker` drops a finished digest only if the epoch moved since dispatch (i.e. the user prompted that session). Every other path (turn-end routing, session-channel announce, held-question fallback, raw fallback) is already correct from PR #12 and is left untouched.

**Tech Stack:** Python 3.14 stdlib, pytest. No new dependencies.

## Global Constraints

- Summary-mode digests are async (`claude -p`, ~6-30 s); correctness must not
  depend on digest latency.
- Both hard rules must hold: (1) each session's most recent *completed* message
  is always read (never dropped by the system's own timing); (2) an
  answer-response is read in full. Both follow from "only the user cancels."
- Resolved: when one session finishes several messages with no user action
  between them, **read all of them, queued** (none dropped).
- Keep FLUSH (new prompt) as a cancel — that is the one legitimate supersede.
- Stdlib only; no new dependencies. No em-dashes in code or docs.

---

### Task 1: Cancel epoch advances only on user action (not on turn-end)

**Files:**
- Modify: `src/sonara/daemon.py` — `_maybe_summarize` (~1042-1044), `_summary_worker` supersede check (~1095-1097), and the two explanatory comments (~101-102, ~481-483).
- Test: `tests/test_daemon_summary_mode.py` — invert one test, add one test, keep the FLUSH regression test.

**Interfaces:**
- Consumes: `_summary_gen: dict` (per-session int epoch), `_start_summary_thread(session, gen, text)`, `_summary_worker(session, gen, text)`, `MsgType.FLUSH` handler.
- Produces: no signature changes. Behavior change only: a turn-end dispatch no longer advances `_summary_gen`; a finished digest is dropped iff `_summary_gen.get(session, 0)` differs from the epoch captured at dispatch.

- [ ] **Step 1: Invert the supersede test and add the queue-all test**

In `tests/test_daemon_summary_mode.py`, replace `test_superseded_worker_result_is_dropped` (currently ~line 450-461) with the two tests below. The old test asserted the removed behavior (a second turn-end dropping the first digest); it is replaced, not kept.

```python
def test_second_turn_end_keeps_first_digest(monkeypatch):
    # User-only cancel (#13): two turn-ends on ONE session with NO user action
    # between them must BOTH be read. A turn merely ending no longer drops the
    # prior turn's finished digest (was: the second turn superseded the first).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # first digest dispatched
    _pad = "This filler sentence carries the turn past the threshold. "
    daemon.handle_message(_prose("fg", "More text. " + _pad * 6, 2, True))
    _turn_done(daemon)                                   # second digest dispatched
    daemon._summarize_fn = lambda text, **kw: "First digest."
    daemon._summary_worker(*calls[0])                    # first result lands late
    ch = daemon.router.channel("fg")
    assert "First digest." in [it.text for it in ch.items[ch.cursor:]]  # NOT dropped


def test_both_queued_digests_play_without_user_action(monkeypatch):
    # "Read all queued" (#13): every finished digest of a session is enqueued when
    # no user action intervenes -- none is superseded by a later turn ending.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # digest 1 dispatched
    _pad = "This filler sentence carries the turn past the threshold. "
    daemon.handle_message(_prose("fg", "Second turn. " + _pad * 6, 2, True))
    _turn_done(daemon)                                   # digest 2 dispatched
    assert len(calls) == 2
    results = iter(["Digest one.", "Digest two."])
    daemon._summarize_fn = lambda text, **kw: next(results)
    daemon._summary_worker(*calls[0])
    daemon._summary_worker(*calls[1])
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert "Digest one." in texts and "Digest two." in texts
```

Leave `test_flush_supersedes_inflight_summary` (~line 486) exactly as-is: it is the regression guard proving a *user* prompt still cancels.

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `python -m pytest tests/test_daemon_summary_mode.py::test_second_turn_end_keeps_first_digest tests/test_daemon_summary_mode.py::test_both_queued_digests_play_without_user_action -v`
Expected: both FAIL. `test_second_turn_end_keeps_first_digest` fails because the current code drops the first digest (the second turn-end bumped `_summary_gen`); `test_both_queued_digests_play_without_user_action` fails because `calls[0]`'s worker sees a bumped epoch and returns early, so "Digest one." is missing.

- [ ] **Step 3: Stop the turn-end dispatch from advancing the epoch**

In `src/sonara/daemon.py`, in `_maybe_summarize`, replace:

```python
        gen = self._summary_gen.get(session, 0) + 1
        self._summary_gen[session] = gen
        self._start_summary_thread(session, gen, text)
        return True                      # async digest in flight -> caller holds
```

with:

```python
        # Capture the session's CANCEL epoch WITHOUT advancing it. Only a user
        # action (a new prompt -> FLUSH) advances the epoch; a turn merely ending
        # must never invalidate a previously-dispatched digest. So several
        # turn-ends with no user action between them each keep their digest (they
        # queue and play) -- the system never drops a finished message (#13).
        gen = self._summary_gen.get(session, 0)
        self._start_summary_thread(session, gen, text)
        return True                      # async digest in flight -> caller holds
```

- [ ] **Step 4: Make the worker drop only on a user-action epoch change**

In `src/sonara/daemon.py`, in `_summary_worker`, replace:

```python
                if self._summary_gen.get(session) != gen:
                    _log("digest dropped: superseded by a newer turn")
                    return               # superseded: a newer turn owns the voice
```

with:

```python
                if self._summary_gen.get(session, 0) != gen:
                    _log("digest dropped: user prompted this session since dispatch")
                    return               # the user moved on -> this reading is cancelled
```

(The `, 0` default keeps the comparison correct now that `_maybe_summarize` no longer seeds the dict: an un-prompted session compares 0 == 0 and proceeds.)

- [ ] **Step 5: Update the two explanatory comments to the new semantics**

In `src/sonara/daemon.py`, replace the class-field comment (~line 101-102):

```python
        # Summary mode: per-session generation counter; a new turn-end supersedes
        # any older in-flight summarizer so a stale result is dropped, not spoken.
```

with:

```python
        # Summary mode: per-session CANCEL epoch. Only a user action (a new prompt
        # -> FLUSH) advances it; a finished digest is dropped iff the epoch moved
        # since it was dispatched. A turn merely ending does NOT advance it, so the
        # system never drops a finished message -- only the user cancels (#13).
```

And replace the FLUSH comment (~line 481-482):

```python
            # A new prompt supersedes any in-flight turn summary: advance the
            # generation so a stale recap is dropped, not spoken into this turn.
```

with:

```python
            # A new prompt is the user cancelling this session: advance the cancel
            # epoch so any digest dispatched before now is dropped when it lands,
            # rather than spoken into the new turn (#13).
```

- [ ] **Step 6: Run the full summary-mode suite**

Run: `python -m pytest tests/test_daemon_summary_mode.py -v`
Expected: all PASS — the two new tests, the untouched `test_flush_supersedes_inflight_summary` (FLUSH still cancels), `test_empty_turn_stays_silent` (sets epoch 1, worker gen 1, proceeds), and every `_summary_worker(*calls[0])` test (epoch defaults 0, gen 0, proceeds).

- [ ] **Step 7: Run the whole test suite for regressions**

Run: `python -m pytest -q`
Expected: no new failures versus the pre-change baseline.

- [ ] **Step 8: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_summary_mode.py
git commit -m "fix(summary): only a user prompt cancels a session's reading (#13)

The cancel epoch (_summary_gen) now advances only on FLUSH (a new prompt),
never on a turn ending. A finished digest is dropped iff the user prompted the
session since dispatch, so the system never drops a completed message and every
queued digest of a session is read. Follow-up to #12.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Deferred (discuss before building)

**Flush-on-answer snappiness.** When the user answers a session's question,
cut off that session's still-playing question/context audio immediately so the
reply-to-the-answer feels like it arrives sooner. The user flagged this as
"not a hard requirement," and it needs its own small design pass (how to detect
"the user answered" — a TOOL message, or first new prose after `_await_choice`,
without cutting the question off before the user has heard it). Both hard rules
are satisfied by Task 1 without it. Raise it as a separate task after Task 1
lands and is verified live.

## Verification note

Task 1 satisfies both hard rules end-to-end:
- **Rule 1** (last message per session): the only path that dropped a completed
  digest was the turn-end epoch bump; removing it means a session's finished
  digest always enqueues and plays (queued behind another session via the
  existing session-channel announce from PR #12).
- **Rule 2** (answer-response): after the user answers, the AI's reply is the
  session's next completed turn; its digest is dropped only if the user prompts
  that session again. Waiting for the reply always reads it in full.
