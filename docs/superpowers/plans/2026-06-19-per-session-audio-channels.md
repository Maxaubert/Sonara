# Per-session Audio Channels Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the daemon's global speech queue + single voice-owner with per-session channels driven by a router, so one session can never wipe, silence, or steal another's audio.

**Architecture:** Each session owns a `SessionChannel` (its current message as an item list + a read cursor that never discards items). A `Router` chooses the single active reader — in auto mode it hands off cooperatively (foreground-first) with a spoken "Session changed" cue; in pin mode it locks to the pinned session and replays from the cursor's start on re-pin. The speak loop pulls one item at a time from the router.

**Tech Stack:** Python 3.9+ stdlib only. pytest. Existing daemon threading model (one `self._lock`, one speak thread).

## Global Constraints

- Python **>= 3.9** (no 3.10+ syntax; use `"str | None"` string annotations as the codebase does).
- Core daemon only — **no** changes to hooks, protocol message types, TTS engine, or hotkey bindings.
- Single-session behavior must stay **identical in feel** — no announcements, no regressions; the existing test suite stays green except where it asserts on removed internals (those tests are migrated, never weakened).
- Diagnostics: the temporary `_qaudit` / `queue_audit.log` instrumentation is removed as part of this work (Task 9).
- Spec: `docs/superpowers/specs/2026-06-19-per-session-audio-channels-design.md`.

---

### Task 1: `SessionChannel` — per-session message buffer with cursor

**Files:**
- Create: `src/sonari/channel.py`
- Test: `tests/test_channel.py`

**Interfaces:**
- Consumes: `SpeechItem` from `sonari.queue`.
- Produces: `SessionChannel(session: str)` with attributes `session, items, cursor, turn_done, muted, has_decision` and methods `append(item)`, `pending() -> int`, `ready(minqueue: int) -> bool`, `caught_up() -> bool`, `peek() -> SpeechItem|None`, `next() -> SpeechItem|None`, `reset() -> None`, `wipe() -> None`.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_channel.py
from sonari.channel import SessionChannel
from sonari.queue import SpeechItem


def _item(text, is_decision=False):
    return SpeechItem(id=0, session="s", kind="prose", text=text, is_decision=is_decision)


def test_append_increases_pending_and_keeps_items():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.append(_item("b"))
    assert ch.pending() == 2 and len(ch.items) == 2 and ch.cursor == 0


def test_next_advances_cursor_without_discarding():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.append(_item("b"))
    assert ch.next().text == "a"
    assert ch.cursor == 1 and len(ch.items) == 2   # item retained for replay
    assert ch.next().text == "b"
    assert ch.next() is None                        # caught up


def test_ready_respects_minqueue_until_turn_done():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.append(_item("b"))
    assert ch.ready(3) is False        # below threshold, turn not done
    ch.turn_done = True
    assert ch.ready(3) is True         # turn done -> flush remainder
    ch.turn_done = False
    ch.append(_item("c"))
    assert ch.ready(3) is True         # reached threshold


def test_ready_true_for_decision_below_threshold():
    ch = SessionChannel("s")
    ch.append(_item("Question?", is_decision=True))
    assert ch.has_decision is True
    assert ch.ready(5) is True         # decisions are readable immediately


def test_reset_replays_from_start():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.append(_item("b"))
    ch.next(); ch.next()
    assert ch.caught_up() is True
    ch.reset()
    assert ch.cursor == 0 and ch.pending() == 2 and ch.next().text == "a"


def test_wipe_clears_everything():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.turn_done = True; ch.next()
    ch.wipe()
    assert ch.items == [] and ch.cursor == 0 and ch.turn_done is False
    assert ch.has_decision is False
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/test_channel.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'sonari.channel'`

- [ ] **Step 3: Implement `SessionChannel`**

```python
# src/sonari/channel.py
"""One session's current message: an item list + a read cursor.

Items are NOT discarded as they are spoken — the cursor advances over them — so a
channel can resume from where it left off (auto hand-off) or replay from the start
(pin re-pin). A new prompt wipes the channel.
"""
from __future__ import annotations

from sonari.queue import SpeechItem


class SessionChannel:
    def __init__(self, session: str) -> None:
        self.session = session
        self.items: "list[SpeechItem]" = []
        self.cursor = 0
        self.turn_done = False
        self.muted = False
        self.has_decision = False   # a user-blocking item is pending -> preempt

    def append(self, item: SpeechItem) -> None:
        self.items.append(item)
        if item.is_decision:
            self.has_decision = True

    def pending(self) -> int:
        return len(self.items) - self.cursor

    def ready(self, minqueue: int) -> bool:
        """True if there is a batch worth reading now: enough buffered, the turn is
        done, or a user-blocking decision is waiting."""
        p = self.pending()
        return p > 0 and (p >= minqueue or self.turn_done or self.has_decision)

    def caught_up(self) -> bool:
        return self.cursor >= len(self.items)

    def peek(self) -> "SpeechItem | None":
        return self.items[self.cursor] if self.cursor < len(self.items) else None

    def next(self) -> "SpeechItem | None":
        if self.cursor >= len(self.items):
            return None
        item = self.items[self.cursor]
        self.cursor += 1
        if self.caught_up():
            self.has_decision = False   # the pending decision has been consumed
        return item

    def reset(self) -> None:
        self.cursor = 0

    def wipe(self) -> None:
        self.items = []
        self.cursor = 0
        self.turn_done = False
        self.has_decision = False
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_channel.py -q`
Expected: PASS (6 passed)

- [ ] **Step 5: Commit**

```bash
git add src/sonari/channel.py tests/test_channel.py
git commit -m "feat(core): SessionChannel — per-session message buffer with cursor (#59)"
```

---

### Task 2: `Router` — active-reader selection + hand-off announcements

**Files:**
- Create: `src/sonari/router.py`
- Test: `tests/test_router.py`

**Interfaces:**
- Consumes: `SessionChannel` (Task 1); a `SessionManager`-like object exposing `pinned() -> str|None`, `foreground() -> str|None`, `folder(session) -> str|None`; a `minqueue` callable `() -> int`; an `announce_text(folder) -> str` callable for the hand-off cue.
- Produces: `Router(sessions, minqueue, announce_text)` with `channel(session) -> SessionChannel`, `drop(session)`, `next_item() -> SpeechItem|None`, `active` attribute, `repin_reset()`.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_router.py
from sonari.router import Router
from sonari.queue import SpeechItem


class FakeSessions:
    def __init__(self): self._pin = None; self._fg = None; self._folders = {}
    def pinned(self): return self._pin
    def foreground(self): return self._fg
    def folder(self, s): return self._folders.get(s)


def _item(session, text, is_decision=False):
    return SpeechItem(id=0, session=session, kind="prose", text=text, is_decision=is_decision)


def _router(mq=1):
    s = FakeSessions()
    r = Router(s, minqueue=lambda: mq, announce_text=lambda f: "Session changed: {0}.".format(f))
    return r, s


def test_single_session_reads_in_order_no_announcement():
    r, s = _router()
    s._fg = "A"
    ch = r.channel("A"); ch.append(_item("A", "one")); ch.append(_item("A", "two")); ch.turn_done = True
    assert r.next_item().text == "one"
    assert r.next_item().text == "two"
    assert r.next_item() is None


def test_auto_handoff_announces_then_reads_foreground_first():
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True
    # A reads its message
    assert r.next_item().text == "a1"
    # B prompts (becomes foreground) with content
    s._fg = "B"
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    # A is caught up -> hand off to B: announcement first, then B's item
    assert r.next_item().text == "Session changed: beta."
    assert r.next_item().text == "b1"
    assert r.next_item() is None


def test_active_reader_finishes_before_handoff():
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A"); a.append(_item("A", "a1")); a.append(_item("A", "a2")); a.turn_done = True
    assert r.next_item().text == "a1"
    # B prompts mid-A-read
    s._fg = "B"; b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    # A keeps the floor until its queue drains (cooperative)
    assert r.next_item().text == "a2"
    assert r.next_item().text == "Session changed: beta."
    assert r.next_item().text == "b1"


def test_muted_channel_is_skipped():
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True; a.muted = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    # A is muted -> B reads (B is the only candidate; announced since active changes)
    item = r.next_item()
    assert item.text in ("Session changed: beta.", "b1")


def test_decision_preempts_current_reader():
    r, s = _router(mq=5)
    s._folders = {"A": "alpha", "B": "beta"}; s._fg = "A"
    a = r.channel("A")
    for i in range(5): a.append(_item("A", "a%d" % i))
    a.turn_done = True
    assert r.next_item().text == "a0"          # A reading
    # B raises a decision
    b = r.channel("B"); b.append(_item("B", "Pick?", is_decision=True))
    nxt = r.next_item()
    assert nxt.text in ("Session changed: beta.", "Pick?")   # preempts to B


def test_pin_locks_and_repin_resets_cursor():
    r, s = _router()
    s._folders = {"A": "alpha", "B": "beta"}
    a = r.channel("A"); a.append(_item("A", "a1")); a.append(_item("A", "a2")); a.turn_done = True
    s._pin = "A"
    assert r.next_item().text == "a1"
    assert r.next_item().text == "a2"
    assert r.next_item() is None               # caught up, pinned -> no handoff
    # re-pin to A replays from the start
    r.repin_reset()
    assert r.next_item().text == "a1"
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/test_router.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'sonari.router'`

- [ ] **Step 3: Implement `Router`**

```python
# src/sonari/router.py
"""Choose the single active reader among per-session channels and yield the next
item to speak. One speaker -> one reader at a time. See the design spec."""
from __future__ import annotations

from sonari.channel import SessionChannel
from sonari.queue import SpeechItem


class Router:
    def __init__(self, sessions, minqueue, announce_text) -> None:
        self.sessions = sessions          # exposes pinned()/foreground()/folder()
        self._minqueue = minqueue          # () -> int
        self._announce_text = announce_text  # (folder) -> str
        self.channels: "dict[str, SessionChannel]" = {}
        self.active: "str | None" = None
        self._announced: "str | None" = None   # session whose hand-off cue we emitted
        self._pending_announce: "str | None" = None

    def channel(self, session: str) -> SessionChannel:
        ch = self.channels.get(session)
        if ch is None:
            ch = SessionChannel(session)
            self.channels[session] = ch
        return ch

    def drop(self, session: str) -> None:
        self.channels.pop(session, None)
        if self.active == session:
            self.active = None
            self._announced = None

    def repin_reset(self) -> None:
        """On a change of pinned target, replay the pinned channel from the start."""
        pinned = self.sessions.pinned()
        if pinned is not None and pinned in self.channels:
            self.channels[pinned].reset()
        self.active = None          # force re-announce/selection
        self._announced = None

    def _ready(self, session: str) -> bool:
        ch = self.channels.get(session)
        return ch is not None and not ch.muted and ch.ready(self._minqueue())

    def _pick(self) -> "str | None":
        pinned = self.sessions.pinned()
        if pinned is not None:
            return pinned if pinned in self.channels else None
        # decisions preempt, even mid-message of another session
        for s, ch in self.channels.items():
            if ch.has_decision and self._ready(s):
                return s
        # the current reader keeps the floor while it still has a batch to read
        if self.active is not None and self._ready(self.active):
            return self.active
        # otherwise: foreground first, then oldest-waiting (insertion order)
        fg = self.sessions.foreground()
        if self._ready(fg):
            return fg
        for s in self.channels:
            if self._ready(s):
                return s
        return None

    def next_item(self) -> "SpeechItem | None":
        # emit a queued hand-off announcement before the new reader's first item
        if self._pending_announce is not None:
            folder = self.sessions.folder(self._pending_announce) or "another session"
            text = self._announce_text(folder)
            self._pending_announce = None
            return SpeechItem(id=0, session=self.active or "", kind="prose",
                              text=text, is_decision=False, mute_exempt=True)
        target = self._pick()
        if target is None:
            self.active = None
            return None
        if target != self.active:
            self.active = target
            # announce in auto mode only, once per becoming-active
            if self.sessions.pinned() is None and self._announced != target:
                self._announced = target
                self._pending_announce = target
                return self.next_item()   # re-enter to emit the cue first
        return self.channels[target].next()
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_router.py -q`
Expected: PASS (6 passed)

- [ ] **Step 5: Commit**

```bash
git add src/sonari/router.py tests/test_router.py
git commit -m "feat(core): Router — per-session active-reader selection + hand-off (#59)"
```

---

### Task 3: Daemon — route inbound messages into channels

**Files:**
- Modify: `src/sonari/daemon.py` (`__init__`, PROSE/TOOL/CHOICE/PLAN/PERMISSION/FLUSH/SESSION_END/turn_done handlers)
- Test: `tests/test_daemon_channels.py`

**Interfaces:**
- Consumes: `Router` (Task 2), `SessionChannel` (Task 1).
- Produces: `self.router: Router` on `SpeechDaemon`; inbound handlers call `self.router.channel(session).append(...)` / `.wipe()` / set `turn_done`; `self.router.drop(session)` on session end.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_daemon_channels.py
from tests.daemon_helpers import make_daemon
from sonari.protocol import MsgType, PROTOCOL_VERSION


def _prose(s, delta, idx, final):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s,
            "delta": delta, "index": idx, "final": final}


def test_prose_lands_in_the_sessions_channel():
    daemon, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Hello there. ", 0, True))
    ch = daemon.router.channel("A")
    assert [i.text for i in ch.items] == ["Hello there."]
    assert ch.turn_done is True


def test_new_prompt_wipes_only_its_own_channel():
    daemon, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "A first. ", 0, True))
    daemon.handle_message(_prose("B", "B first. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "A"})
    assert daemon.router.channel("A").items == []          # A wiped
    assert [i.text for i in daemon.router.channel("B").items] == ["B first."]  # B intact
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python -m pytest tests/test_daemon_channels.py -q`
Expected: FAIL — `AttributeError: 'SpeechDaemon' object has no attribute 'router'`

- [ ] **Step 3: Construct the router in `__init__` and route messages**

In `SpeechDaemon.__init__`, after `self.sessions = sessions`, add:

```python
        from sonari.router import Router
        self.router = Router(
            self.sessions,
            minqueue=self._minqueue,
            announce_text=lambda folder: "Session changed: {0}.".format(folder),
        )
```

Replace the PROSE handler body so prose/tool/decision append to the session's
channel instead of buffering/enqueuing. New PROSE branch:

```python
        if t == MsgType.PROSE:
            final = msg.get("final", False)
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), final)
            from sonari.assembler import PARAGRAPH_BREAK
            ch = self.router.channel(session)
            for chunk in chunks:
                if chunk is PARAGRAPH_BREAK:
                    self.history.end_message(session)
                    continue
                entry = self.history.record(session, "prose", chunk)
                item = SpeechItem(id=self._alloc_id(), session=session, kind="prose",
                                  text=chunk, is_decision=False)
                self._pending_heard[item.id] = entry
                ch.append(item)
            if final:
                ch.turn_done = True
                self.history.end_message(session)
                self._options.pop(session, None)
            self._wake.set()
            return None
```

In the FLUSH handler, replace the queue/buffer/voice teardown with a channel wipe
(keep pause-clear + cancel-current). New FLUSH branch:

```python
        if t == MsgType.FLUSH:
            cur = self._current_item
            if cur is not None and cur.session == session:
                self.speaker.cancel()
            self.router.channel(session).wipe()
            self._assemblers.pop(session, None)
            self.history.reset(session)
            self._nav_cursor.pop(session, None)
            self._paused.clear()
            self._wake.set()
            self._options.pop(session, None)
            return None
```

In SESSION_END, replace the queue flush with `self.router.drop(session)` (keep
history reset + the discard calls that still apply). New SESSION_END:

```python
        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self.router.drop(session)
            self.history.reset(session)
            self._options.pop(session, None)
            self._warned_immediate.discard(session)
            self._guided_sessions.discard(session)
            return None
```

Route TOOL and CHOICE/PLAN/PERMISSION through the channel too. TOOL:

```python
        if t == MsgType.TOOL:
            if verbosity == "everything":
                tool = msg.get("tool", "")
                summary = (msg.get("summary") or "").strip()
                text = summary if summary else "Running {0}.".format(tool)
                self.router.channel(session).append(SpeechItem(
                    id=self._alloc_id(), session=session, kind="tool_announce",
                    text=text, is_decision=False))
                self._wake.set()
            return None
```

CHOICE (PLAN/PERMISSION mirror it — append a decision item; the EARCON branch is
unchanged and still fires the alert):

```python
        if t == MsgType.CHOICE:
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
            self.router.channel(session).append(item)
            self._wake.set()
            return None
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python -m pytest tests/test_daemon_channels.py -q`
Expected: PASS (2 passed)

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_channels.py
git commit -m "feat(core): route inbound messages into per-session channels (#59)"
```

---

### Task 4: Daemon — speak loop pulls from the router

**Files:**
- Modify: `src/sonari/daemon.py` (`_speak_loop_once`, `note_spoken`); delete `_voice_owner`, `_may_speak`, `_claim_for_decision`, `_owner_mid_reply`, `_captured_msg`, `_open_msg`, `_prose_buffer` and their references; remove `self.queue` usage.
- Test: `tests/test_daemon_loop.py` (migrate existing), `tests/test_daemon_multisession.py` (Task 9 adds more)

**Interfaces:**
- Consumes: `self.router.next_item()`.
- Produces: a speak loop that speaks `router.next_item()` under the lock; `note_spoken` no longer manages voice ownership.

- [ ] **Step 1: Write the failing test**

```python
# add to tests/test_daemon_channels.py
def test_speak_loop_reads_active_channel_then_idles():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "One. Two. ", 0, True))
    daemon._speak_loop_once()
    daemon._speak_loop_once()
    assert speaker.spoken == ["One.", "Two."]
    daemon._speak_loop_once()                 # nothing left -> idle, no error
    assert speaker.spoken == ["One.", "Two."]
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python -m pytest tests/test_daemon_channels.py::test_speak_loop_reads_active_channel_then_idles -q`
Expected: FAIL (speak loop still pops from the removed `self.queue`)

- [ ] **Step 3: Rewrite `_speak_loop_once` and `note_spoken`**

```python
    def _speak_loop_once(self) -> None:
        if self._paused.is_set():
            self._wake.wait(self._poll_interval)
            self._wake.clear()
            return
        with self._lock:
            item = self.router.next_item()
            self._current_item = item
            cancel_epoch = self.speaker.cancel_epoch()
            muted = False   # router already skips muted channels
        if item is None:
            self._wake.wait(self._poll_interval)
            self._wake.clear()
            return
        try:
            completed = self.speaker.speak(item.text, cancel_epoch=cancel_epoch)
        except Exception:  # noqa: BLE001
            completed = False
        requeued = False
        with self._lock:
            if not completed and self._paused.is_set():
                # paused mid-utterance: rewind the cursor so resume re-speaks it
                ch = self.router.channels.get(item.session)
                if ch is not None and ch.cursor > 0:
                    ch.cursor -= 1
                self._current_item = None
                requeued = True
        if not requeued:
            self.note_spoken(item, completed)
```

```python
    def note_spoken(self, item, completed: bool) -> None:
        with self._lock:
            self._current_item = None
            entry = self._pending_heard.pop(item.id, None)
            if entry is not None and completed:
                entry.heard = True
```

Delete the methods `_may_speak`, `_claim_for_decision`, `_owner_mid_reply`,
`_buffer_prose`, `_flush_prose_buffer`, and every reference to `self._voice_owner`,
`self._captured_msg`, `self._open_msg`, `self._prose_buffer`, and `self.queue`
throughout `daemon.py` (including in `__init__`, FLUSH, STOP, MUTE, PAUSE, NAV,
CATCH_UP, REPEAT, the EARCON `turn_done` branch, and the idle release).

For STOP, clear every channel:

```python
        if t == MsgType.STOP:
            for ch in self.router.channels.values():
                ch.wipe()
            self.speaker.cancel()
            return None
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_daemon_channels.py -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_channels.py
git commit -m "feat(core): speak loop pulls from the router; drop global voice-owner (#59)"
```

---

### Task 5: Pause (single full-silence halt) + per-session mute on the active reader

**Files:**
- Modify: `src/sonari/daemon.py` (PAUSE, MUTE handlers)
- Test: `tests/test_daemon_pause_mute.py` (migrate)

**Interfaces:**
- Consumes: `self.router.active`, `self.router.channel(session)`.
- Produces: PAUSE toggles `self._paused` (cancel current, re-speak on resume via cursor rewind from Task 4); MUTE toggles `channel.muted` for the **active reader**.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_daemon_pause_mute.py (new cases; old voice-owner cases are removed)
from tests.daemon_helpers import make_daemon
from sonari.protocol import MsgType, PROTOCOL_VERSION


def _prose(s, d, i, f):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s, "delta": d, "index": i, "final": f}


def test_mute_targets_active_reader_and_skips_it():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Secret one. Secret two. ", 0, True))
    daemon._speak_loop_once()                       # speaks "Secret one." -> A is active
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert daemon.router.channel("A").muted is True
    speaker.spoken.clear()
    daemon._speak_loop_once()                       # A muted -> skipped -> idle
    assert speaker.spoken == []


def test_pause_halts_then_resumes_same_item():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Alpha. Beta. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})  # pause
    daemon._speak_loop_once()
    assert speaker.spoken == []                     # held
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})  # resume
    daemon._speak_loop_once()
    assert speaker.spoken == ["Alpha."]
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/test_daemon_pause_mute.py -q`
Expected: FAIL (MUTE still references `_muted_sessions`/foreground)

- [ ] **Step 3: Rewrite PAUSE and MUTE**

```python
        if t == MsgType.PAUSE:
            if self._paused.is_set():
                self._paused.clear()
                self._wake.set()
                fg = self.router.active or self.sessions.foreground()
                if fg is not None:
                    self._speak_cue(fg, "Resumed.")
            else:
                self._paused.set()
                self.speaker.cancel()
                fg = self.router.active or self.sessions.foreground()
                if fg is not None:
                    self._speak_cue(fg, "Paused.", pause_exempt=True)
            return None

        if t == MsgType.MUTE:
            target = self.router.active or self.sessions.foreground()
            if target is None:
                return None
            ch = self.router.channel(target)
            ch.muted = not ch.muted
            if ch.muted and self._current_item is not None \
                    and self._current_item.session == target:
                self.speaker.cancel()
            word = "muted" if ch.muted else "unmuted"
            self._speak_cue(target, "Session {0}.".format(word), exempt_mute=True)
            return None
```

Add a small helper `_speak_cue` that appends a one-off exempt cue to the front of a
channel so confirmations are always heard (implementation detail: insert a cue item
at the cursor so it speaks next; mark `mute_exempt`/`pause_exempt` so the loop/router
honor it). Pause cues must play during the global halt — keep the paused-branch
override from the existing pause-cue feature: when paused, still emit a pending
`pause_exempt` cue before holding.

```python
    def _speak_cue(self, session: str, text: str, exempt_mute: bool = False,
                   pause_exempt: bool = False) -> None:
        item = SpeechItem(id=self._alloc_id(), session=session, kind="prose",
                          text=text, is_decision=False, mute_exempt=exempt_mute,
                          pause_exempt=pause_exempt)
        ch = self.router.channel(session)
        ch.items.insert(ch.cursor, item)   # speak next, then continue
        self._wake.set()
```

(Adjust the paused branch of `_speak_loop_once` to drain a `pause_exempt` cue at the
cursor before holding — mirrors the shipped "Paused." behavior.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_daemon_pause_mute.py -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_pause_mute.py
git commit -m "feat(core): pause = full silence; mute targets the active reader (#59)"
```

---

### Task 6: Pin / re-pin replay

**Files:**
- Modify: `src/sonari/daemon.py` (PIN_TOGGLE handler)
- Test: `tests/test_daemon_pin.py` (migrate)

**Interfaces:**
- Consumes: `self.router.repin_reset()`.
- Produces: pin_toggle that, on a *change* of pinned target, calls `self.router.repin_reset()` so the pinned session replays from the start.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_daemon_pin.py
def test_repin_replays_pinned_channel_from_start():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "One. Two. ", 0, True))
    daemon._speak_loop_once(); daemon._speak_loop_once()   # reads One., Two.
    speaker.spoken.clear()
    sessions.set_foreground("A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PIN_TOGGLE})  # pin A
    daemon._speak_loop_once()
    assert speaker.spoken[-1] == "One."     # replayed from the start
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python -m pytest tests/test_daemon_pin.py::test_repin_replays_pinned_channel_from_start -q`
Expected: FAIL (no replay)

- [ ] **Step 3: Call `repin_reset()` in the pin handler**

In the `PIN_TOGGLE` handler, after `action, folder = self.sessions.pin_toggle()`, when
`action == "pinned"` add `self.router.repin_reset()` before emitting the cue. Keep the
"Pinned {folder}." / "Auto." cues, emitted via `_speak_cue` on the foreground.

- [ ] **Step 4: Run test to verify it passes**

Run: `python -m pytest tests/test_daemon_pin.py::test_repin_replays_pinned_channel_from_start -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_pin.py
git commit -m "feat(core): re-pin replays the pinned channel from the start (#59)"
```

---

### Task 7: Migrate nav / repeat / catch-up / reread to channels

**Files:**
- Modify: `src/sonari/daemon.py` (`_nav`, REPEAT, CATCH_UP, REREAD_OPTIONS, JUMP_DECISION)
- Test: `tests/test_daemon_nav.py`, `tests/test_daemon_decisions.py` (migrate)

**Interfaces:**
- Consumes: `self.history` (unchanged) + `self.router.channel(session)`.
- Produces: nav/repeat/catch-up enqueue replay items by inserting them into the target session's channel at its cursor (so they read next via the router), rather than into the removed global queue.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_daemon_nav.py
def test_repeat_reads_last_message_via_channel():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Hello. ", 0, True))
    daemon._speak_loop_once()                     # reads "Hello."
    speaker.spoken.clear()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.REPEAT})
    daemon._speak_loop_once()
    assert speaker.spoken == ["Hello."]
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python -m pytest tests/test_daemon_nav.py::test_repeat_reads_last_message_via_channel -q`
Expected: FAIL (REPEAT enqueues to the removed queue)

- [ ] **Step 3: Reimplement replay actions over channels**

Add a helper that inserts replay items at the active cursor of a session's channel:

```python
    def _replay(self, session: str, entries) -> None:
        ch = self.router.channel(session)
        at = ch.cursor
        for e in entries:
            item = SpeechItem(id=self._alloc_id(), session=session, kind=e.kind,
                              text=e.text, is_decision=e.kind in ("choice", "plan", "permission"))
            self._pending_heard[item.id] = e
            ch.items.insert(at, item)
            at += 1
        self._wake.set()
```

Rewrite REPEAT to `self._replay(fg, self.history.last_message(fg))` (or the
"Nothing to repeat." cue via `_speak_cue`). Rewrite REREAD_OPTIONS to `_speak_cue`
the stored options text. Rewrite CATCH_UP to `_replay` the unheard entries into the
target session's channel and `repin`/activate that session. Rewrite `_nav` to seek
the channel cursor / `_replay` the target message. JUMP_DECISION advances the active
channel cursor to the next decision item.

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_daemon_nav.py tests/test_daemon_decisions.py -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/test_daemon_nav.py tests/test_daemon_decisions.py
git commit -m "feat(core): nav/repeat/catch-up/reread replay via channels (#59)"
```

---

### Task 8: Remove dead code (`SpeechQueue`, audit instrumentation) + green the suite

**Files:**
- Modify: `src/sonari/queue.py` (drop `SpeechQueue`; keep `SpeechItem`), `src/sonari/daemon.py` (remove `_qaudit`, `SpeechQueue` import, `self.queue`), `tests/test_queue.py` (drop `SpeechQueue` tests), `tests/daemon_helpers.py` (stop passing a queue), and any remaining `tests/test_daemon_*` that assert on removed internals.

**Interfaces:**
- Consumes: nothing new.
- Produces: a daemon with no global queue, no `_qaudit`, no `queue_audit.log`.

- [ ] **Step 1: Delete `SpeechQueue` and the `_qaudit` instrumentation**

Remove the `SpeechQueue` class from `queue.py` (keep `SpeechItem`). Remove `_qaudit`
and all its call sites from `daemon.py`. Update `make_daemon` in
`tests/daemon_helpers.py` to construct `SpeechDaemon` without a `SpeechQueue` (pass
whatever the new constructor needs).

- [ ] **Step 2: Run the full suite**

Run: `python -m pytest -q`
Expected: failures only in tests that asserted on removed internals (voice-owner,
queue length). Migrate each to the channel/router equivalent — never weaken an
assertion. Re-run until green (excluding the known environmental Windows failures:
`test_bin_shims`, `test_bin_sonari`, `test_daemon_main::test_ensure_running...`,
`test_paths`, `test_transport`, `test_win_autostart`, `test_win_tts`).

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(core): remove SpeechQueue + audit instrumentation; migrate tests (#59)"
```

---

### Task 9: Multi-session integration tests (the bug report)

**Files:**
- Create: `tests/test_daemon_multisession.py`

**Interfaces:**
- Consumes: the full daemon.

- [ ] **Step 1: Write the integration tests**

```python
# tests/test_daemon_multisession.py
from tests.daemon_helpers import make_daemon
from sonari.protocol import MsgType, PROTOCOL_VERSION


def _prose(s, d, i, f):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s, "delta": d, "index": i, "final": f}


def _fg(daemon, s):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": s})
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": s})


def test_pausing_one_session_does_not_lose_anothers_speech():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    _fg(daemon, "B")
    daemon.handle_message(_prose("B", "B speaks. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})  # pause (B active)
    _fg(daemon, "A")                                                        # switch to A (clears pause)
    daemon.handle_message(_prose("A", "A speaks. ", 0, True))
    daemon._speak_loop_once()
    assert "A speaks." in speaker.spoken          # A is heard; not stuck behind B


def test_two_sessions_take_turns_nothing_lost():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    sessions._folders = {"A": "alpha", "B": "beta"} if hasattr(sessions, "_folders") else None
    daemon.handle_message(_prose("A", "A one. ", 0, True))
    _fg(daemon, "B")
    daemon.handle_message(_prose("B", "B one. ", 0, True))
    for _ in range(6):
        daemon._speak_loop_once()
    assert "A one." in speaker.spoken and "B one." in speaker.spoken
```

- [ ] **Step 2: Run to verify (write, watch fail if behavior is wrong, fix, pass)**

Run: `python -m pytest tests/test_daemon_multisession.py -q`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/test_daemon_multisession.py
git commit -m "test(core): multi-session channel integration (the #59 repro) (#59)"
```

---

## Self-review notes

- Spec §3 (`SessionChannel`, `Router`) → Tasks 1-2. §4 behaviors → Tasks 3-7. §5
  removals → Tasks 4, 8. §6 edge cases → Tasks 4 (flush active), 5 (mute/pause), 9
  (single + multi session). §7 nav scope → Task 7. §8 testing → every task + 9. §9
  rollout (remove instrumentation) → Task 8.
- `SpeechItem` gains use of its existing `mute_exempt`/`pause_exempt` fields; no new
  fields needed.
- After Task 8, verify live on Windows by cherry-picking onto the running daemon
  branch and replaying the repro before opening the PR.
