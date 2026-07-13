# Turn-End Settle Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In summary mode, defer a turn's digest until its prose has settled, so multi-session content is never read partially or dropped.

**Architecture:** `turn_done` arms a short per-session settle window instead of digesting immediately. Every PROSE delta for a session with a window pending resets it. When the window elapses quiet, a timer thread takes `self._lock` and calls `_maybe_summarize` on the complete turn. A new prompt (`FLUSH`) cancels the window. A per-session generation guard makes a fire that races a re-arm or cancel a no-op.

**Tech Stack:** Python 3.14 stdlib (`threading.Timer`), pytest.

## Global Constraints

- Summary-mode digests are async (~6-30s); a sub-second settle is invisible for
  digested turns.
- Rule 1 (from #13): the system must never drop content. This fix closes the
  turn-boundary race that stranded content.
- The timer fires off the message-handling path, which is the only path that
  does NOT already hold `self._lock`; the fire must acquire the lock itself.
  `_maybe_summarize` and its callees never take the lock.
- Default settle window: 600 ms, configurable via `summary_settle_ms`.
- Scope: `turn_done` path only. Decision paths (`CHOICE`/`PLAN`/`PERMISSION`)
  are out of scope. No em-dashes.

---

### Task 1: Per-session settle window before the turn-end digest

**Files:**
- Modify: `src/sonara/daemon.py` — new state in `__init__` (~line 103 area), new methods `_arm_settle`/`_settle_schedule`/`_settle_fire`/`_cancel_settle`, the `turn_done` earcon handler (~464-470), the PROSE handler tail (~353-368), the `FLUSH` handler (~473-492), and `SESSION_END` cleanup (~511-520).
- Test: `tests/test_daemon_summary_mode.py` — update the `_turn_done` helper to fire the settle deterministically; add settle-specific tests.

**Interfaces:**
- Consumes: `_maybe_summarize(session)`, `self._lock`, `self.config`, `self.router.channel(session).turn_done`, `MsgType.EARCON`/`PROSE`/`FLUSH`/`SESSION_END` handlers, `_summary_gen` (from #13, unaffected).
- Produces: `_arm_settle(session)`, `_settle_schedule(session, gen)` (test seam), `_settle_fire(session, gen)`, `_cancel_settle(session)`; new state `_settle_timers: dict`, `_settle_gen: dict`, `_settle_pending: set`; new config key `summary_settle_ms` (default 600).

- [ ] **Step 1: Update the `_turn_done` test helper and add settle tests (failing)**

In `tests/test_daemon_summary_mode.py`, replace the `_turn_done` helper (currently ~line 77-79) so it fires the settle window deterministically instead of relying on the real 600 ms timer:

```python
def _turn_done(daemon, session="fg"):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": session})
    _fire_settle(daemon, session)


def _fire_settle(daemon, session="fg"):
    # Deterministic settle: cancel the real timer and fire synchronously, so
    # turn-end tests do not wait on the clock (#14).
    gen = daemon._settle_gen.get(session)
    if gen is None:
        return
    t = daemon._settle_timers.pop(session, None)
    if t is not None:
        t.cancel()
    daemon._settle_fire(session, gen)
```

Then add these settle-specific tests at the end of the file:

```python
def test_turn_done_defers_digest_until_settle(monkeypatch):
    # turn_done alone must NOT dispatch: the turn's final prose may still be in
    # flight (separate hook processes race). Only the settle fire dispatches (#14).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    _enable_and_feed(daemon, monkeypatch)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    assert calls == []                       # deferred: no digest yet
    assert scheduled and scheduled[-1][0] == "fg"
    daemon._settle_fire("fg", scheduled[-1][1])
    assert len(calls) == 1                    # settle fired -> dispatched


def test_late_prose_included_after_turn_done(monkeypatch):
    # The bug: turn_done arrives before the paragraph body. With the settle
    # window, the late body is included and the turn takes the digest path (#14).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Here is another one:", 0, True))   # short lead-in
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})           # arms window
    _pad = "This filler sentence carries the turn well past the threshold. "
    daemon.handle_message(_prose("fg", " " + _pad * 6, 1, True))            # late body -> re-arm
    daemon._settle_fire("fg", scheduled[-1][1])                            # window fires
    assert len(calls) == 1
    _, _, text = calls[0]
    assert "Here is another one:" in text and "filler sentence" in text     # FULL turn digested


def test_settle_window_resets_on_new_prose(monkeypatch):
    # New prose after turn_done re-arms the window; a fire from the superseded
    # (earlier) window is a no-op, only the latest fire dispatches (#14).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    _enable_and_feed(daemon, monkeypatch)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    first_gen = scheduled[-1][1]
    _pad = "This filler sentence carries the turn well past the threshold. "
    daemon.handle_message(_prose("fg", " " + _pad * 6, 2, True))            # re-arms
    second_gen = scheduled[-1][1]
    assert second_gen != first_gen
    daemon._settle_fire("fg", first_gen)                                    # stale
    assert calls == []
    daemon._settle_fire("fg", second_gen)                                  # current
    assert len(calls) == 1


def test_flush_cancels_pending_settle(monkeypatch):
    # A new prompt during the settle window abandons the turn: a late fire from
    # the cancelled window dispatches nothing (#14, consistent with #13).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    _enable_and_feed(daemon, monkeypatch)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    stale_gen = scheduled[-1][1]
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    daemon._settle_fire("fg", stale_gen)
    assert calls == []
    assert "fg" not in daemon._settle_pending
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `python -m pytest tests/test_daemon_summary_mode.py -k "settle or late_prose" -v`
Expected: FAIL with `AttributeError` on `_settle_gen`/`_settle_schedule`/`_settle_fire`/`_settle_pending` (not defined yet).

- [ ] **Step 3: Add settle state to `__init__`**

In `src/sonara/daemon.py`, right after the `_summary_gen` block (~line 107), add:

```python
        # Summary mode: per-session turn-end SETTLE window (#14). turn_done can
        # reach the daemon before a turn's final prose (separate hook processes
        # race under multi-session load), so digesting immediately summarizes an
        # incomplete/empty turn. Arm a short window on turn_done, reset it on each
        # new prose delta, and digest only once the session is quiet.
        self._settle_timers: dict = {}   # session -> threading.Timer
        self._settle_gen: dict = {}      # session -> int (stale-fire guard)
        self._settle_pending: set = set()  # sessions with a window armed
```

- [ ] **Step 4: Add the settle methods**

In `src/sonara/daemon.py`, add these methods next to `_maybe_summarize` (after it, ~line 1060+):

```python
    def _arm_settle(self, session: str) -> None:
        """Defer the turn-end digest until the session's prose settles. Restart the
        window on every new prose delta; fire once quiet (#14). Caller holds the
        lock (this runs from handle_message)."""
        gen = self._settle_gen.get(session, 0) + 1
        self._settle_gen[session] = gen
        self._settle_pending.add(session)
        old = self._settle_timers.pop(session, None)
        if old is not None:
            old.cancel()
        self._settle_schedule(session, gen)

    def _settle_schedule(self, session: str, gen: int) -> None:
        """Start the real settle timer. Test seam: tests replace this to drive
        _settle_fire deterministically instead of waiting on the clock."""
        settle_s = self.config.get("summary_settle_ms", 600) / 1000.0
        t = threading.Timer(settle_s, self._settle_fire, args=(session, gen))
        t.daemon = True
        self._settle_timers[session] = t
        t.start()

    def _settle_fire(self, session: str, gen: int) -> None:
        """The settle window elapsed with no new prose: dispatch the turn-end
        digest now that the full turn has landed. Runs on the Timer thread, so it
        takes the lock. A stale fire (re-armed by later prose, or cancelled by
        FLUSH) is a no-op via the generation guard."""
        with self._lock:
            if self._settle_gen.get(session) != gen:
                return
            self._settle_pending.discard(session)
            self._settle_timers.pop(session, None)
            self._maybe_summarize(session)

    def _cancel_settle(self, session: str) -> None:
        """Drop any pending settle window: a new prompt abandons the turn. Bumps
        the generation so an already-scheduled fire becomes a no-op."""
        self._settle_pending.discard(session)
        self._settle_gen[session] = self._settle_gen.get(session, 0) + 1
        t = self._settle_timers.pop(session, None)
        if t is not None:
            t.cancel()
```

- [ ] **Step 5: Arm the settle window on `turn_done` instead of digesting immediately**

In `src/sonara/daemon.py`, in the `turn_done` earcon handler, replace:

```python
                self.router.channel(session).turn_done = True
                self._wake.set()
                self._maybe_summarize(session)
```

with:

```python
                self.router.channel(session).turn_done = True
                self._wake.set()
                # Do NOT digest yet: the turn's final prose can arrive after this
                # signal (separate hook processes race). Arm a settle window and
                # digest once the session is quiet (#14). Non-summary mode has no
                # digest, so nothing to defer there.
                if self.config.get("summary_mode"):
                    self._arm_settle(session)
```

- [ ] **Step 6: Reset the window on late prose**

In `src/sonara/daemon.py`, in the PROSE handler, just before the final `if ch.ready(self._minqueue()):` wake, add:

```python
            # Late prose after turn_done: reset the settle window so the turn-end
            # digest waits for the full turn to land (#14). Only when armed.
            if session in self._settle_pending:
                self._arm_settle(session)
```

- [ ] **Step 7: Cancel the window on FLUSH and SESSION_END**

In `src/sonara/daemon.py`, in the `FLUSH` handler, next to the other per-session resets (near the `_summary_gen` bump ~line 483), add:

```python
            self._cancel_settle(session)      # new prompt abandons any settling turn (#14)
```

And in the `SESSION_END` handler, next to `self._summary_gen.pop(session, None)`, add:

```python
            self._cancel_settle(session)
            self._settle_gen.pop(session, None)
```

- [ ] **Step 8: Run the settle tests (green)**

Run: `python -m pytest tests/test_daemon_summary_mode.py -k "settle or late_prose" -v`
Expected: all PASS.

- [ ] **Step 9: Run the full summary-mode suite**

Run: `python -m pytest tests/test_daemon_summary_mode.py -q`
Expected: all PASS. Existing turn-end tests pass because the updated `_turn_done` helper fires the settle deterministically.

- [ ] **Step 10: Run the whole suite for regressions**

Run: `python -m pytest -q`
Expected: only the 10 known pre-existing environmental failures (Win32 shim, winrt/winsound mocks, filesystem paths, lockfile perms) — no new failures.

- [ ] **Step 11: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_summary_mode.py
git commit -m "fix(summary): settle window before turn-end digest (#14)

turn_done can reach the daemon before a turn's final prose (separate hook
processes race under multi-session load), so summarizing immediately digested an
incomplete or empty turn and stranded the late text. Arm a short per-session
settle window on turn_done, reset it on each new prose delta, and digest only
once the session is quiet. FLUSH cancels the window. Default 600 ms via
summary_settle_ms. Follow-up to #13.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

## Note on diagnostic logging

The uncommitted `_seq` playback-order logging stays in the working tree during
this task for live verification, and is removed before the branch PR.
