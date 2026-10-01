# Sonari Phase 2.1 - Eyes-free Prompt Interaction - Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the three queue-tail re-speak hotkeys (`reread_options`, `repeat`, `catch_up`) on a new per-session narration-history substrate, add arrow-key caret tracking with a virtual Submit for multi-selects, and encode the voice-continuity rule - per the approved spec `docs/phase-2.1-eyes-free-prompts-spec.md`.

**Architecture:** One new pure module (`src/sonari/history.py`: per-session rolling history + sentence-granular heard-marker) wired into `SpeechDaemon`. "Heard" is confirmed by the speak loop using `say`'s exit code (0 = sentence completed; terminated = stays unheard, so replays restart at the sentence start). Voice continuity = a `_voice_owner` the speak loop releases on drain; a free voice is acquired only by the foreground session at a message boundary; everything else is captured silently into history. Caret tracking = a **listen-only** `CGEventTap` in the Swift hotkeyd (never consumes - the TUI still gets every arrow) that forwards arrow presses to the daemon **only while** a `~/.sonari/prompt-open` flag file (written by the daemon) exists; the daemon mirrors the cursor over the option list it already holds.

**Tech Stack:** Python 3.9 stdlib only (`src/sonari/`), pytest (tests must pass on system `/usr/bin/python3` 3.9), Swift/Carbon/CoreGraphics for hotkeyd. Daemon runs from `~/.sonari/app` - code changes reach the live daemon only after `sonari install`.

**STATUS (2026-06-10):** Built + deployed, EXCEPT **Tasks 8–10 (caret tracking), which were built then REMOVED/de-scoped.** A macOS event tap needs Secure Keyboard Entry OFF *and* an Input-Monitoring grant that resets on every unsigned rebuild (the grant is cdhash-bound; only code-signing makes it persist) - not shippable. The tap also must run on its own `CFRunLoopRun()` thread, not the main loop under `NSApplication.run()`, or it installs but delivers no events. Kept: the history substrate, `repeat`, `catch_up`, `reread_options` (+ a fix to read the permission `message`), and voice continuity. The eyes-free multi-select **Submit** gap remains open.

**Resolved spec assumptions (verified against the code - do not re-investigate):**
- Session identity: every hook payload carries `session_id`; the daemon already keys assemblers and foreground on it (`hooks_entry.py:33`).
- "Focused session" = `sessions.foreground()`, set by `SessionStart` + `UserPromptSubmit` (there is no OS-window-focus hook). The "come back to A and hear what I missed" scenario is covered by catch_up's cross-session fallback (Task 7), not window tracking.
- Sentence-granular heard-marker: `Speaker.speak` runs one `say` per sentence and `cancel()` terminates it → `proc.returncode == 0` iff the sentence completed (Task 1).
- Arrow coexistence: `kCGEventTapOptionListenOnly` observes without consuming. It requires the one-time **Input Monitoring** permission; everything else works without it (graceful degrade, doctor check, request once).

**Deviations from spec (deliberate, justified):**
- The optional *"N new here"* refocus cue is **dropped** (YAGNI): without an OS-focus signal there is no "refocus" event to hook it to, and a new prompt resets the backlog anyway.
- `skip` marks the skipped sentence **heard** (the user deliberately discarded it; catch_up must not nag with it). `stop` leaves everything unheard (recoverable) per spec.
- Caret tracking arms only for **single-question** CHOICE prompts (multi-question Tab navigation would desync the mirror; the TUI shows one question's options at a time). Permission/plan prompts hold no structured option list (the hook payload has only `action`/plan text), so no caret there - consistent with the spec's "prompts Sonari already knows" framing.
- `tool_announce` lines are **not** recorded in history (catch_up replaying "Running Bash" noise would be hostile to the listener).
- **Voice-owner release on mid-response pauses** (declared post-review): the owner releases the voice whenever its queue drains - including a long tool-call pause *inside* a response. If you have since prompted another session, the rest of the paused response is captured silently rather than resuming aloud. This reads the locked rule "the voice was busy when its response landed → captured" at message granularity: in single-session use behavior is unchanged (the still-foreground owner re-acquires instantly). **Verify by ear in T12 step 5**; if it feels wrong live, a message-in-flight ownership hold is the follow-up.

---

### Task 0: Branch + commit the spec

**Files:** none modified (git only). The spec `docs/phase-2.1-eyes-free-prompts-spec.md` is currently untracked.

- [ ] **Step 1: Create the work branch off main**

```bash
cd ~/projects/private/claude-tts
git checkout -b phase-2.1-eyes-free-prompts
```

- [ ] **Step 2: Commit the spec (only the spec - leave `docs/getting-started.md` untracked, it's a separate in-review draft)**

```bash
git add docs/phase-2.1-eyes-free-prompts-spec.md
git commit -m "docs: Phase 2.1 spec - eyes-free prompt interaction (history substrate, repeat/catch_up/reread fixes, caret tracking, voice continuity)"
```

- [ ] **Step 3: Baseline - full suite green before touching code**

Run: `python3 -m pytest -q`
Expected: 362 passed (or current count), 0 failures.

---

### Task 1: `Speaker.speak` returns whether the sentence completed

**Files:**
- Modify: `src/sonari/speaker.py:90-103`
- Modify: `tests/daemon_helpers.py` (FakeSpeaker)
- Test: `tests/test_speaker.py` (append)

`say` exits 0 when it finishes the utterance; `cancel()`/timeout terminate it (non-zero / -15). This return value is the sentence-granular "heard" signal.

- [ ] **Step 1: Write the failing tests** (append to `tests/test_speaker.py`; match its existing fake-runner style - read the file's first fake to reuse its pattern):

```python
class _DoneProc:
    returncode = 0
    def wait(self, timeout=None):
        return 0
    def terminate(self):
        self.returncode = -15


class _KilledProc:
    returncode = None
    def wait(self, timeout=None):
        self.returncode = -15
        return -15
    def terminate(self):
        self.returncode = -15


def test_speak_returns_true_when_say_completes():
    from sonari.speaker import Speaker
    s = Speaker(say_runner=lambda text, voice, rate: _DoneProc())
    assert s.speak("Hello there.") is True


def test_speak_returns_false_when_say_terminated():
    from sonari.speaker import Speaker
    s = Speaker(say_runner=lambda text, voice, rate: _KilledProc())
    assert s.speak("Hello there.") is False
```

- [ ] **Step 2: Run to verify they fail**

Run: `python3 -m pytest tests/test_speaker.py -q -k "returns"`
Expected: FAIL - `speak` currently returns `None`.

- [ ] **Step 3: Implement** - change `Speaker.speak` (keep everything else identical):

```python
    def speak(self, text: str) -> bool:
        """Speak text, blocking. Return True iff the utterance COMPLETED
        (say exited 0). A cancelled/terminated utterance returns False so the
        caller can leave it marked unheard (sentence-granular replay)."""
        proc = self._say_runner(text, self._voice, self._rate)
        with self._current_lock:
            self._current = proc
        try:
            try:
                proc.wait(timeout=self._wait_timeout)
            except subprocess.TimeoutExpired:
                # 'say' hung past the generous deadline; kill it and move on.
                proc.terminate()
        finally:
            with self._current_lock:
                if self._current is proc:
                    self._current = None
        return getattr(proc, "returncode", None) == 0
```

- [ ] **Step 4: Update `tests/daemon_helpers.py` FakeSpeaker** so daemon tests can drive both outcomes:

```python
    def __init__(self):
        self.spoken: list[str] = []
        self.earcons: list[str] = []
        self.cancels: int = 0
        self.rates: list[int] = []
        self.voices: list = []
        self.complete = True          # next speak() reports completed?

    def speak(self, text: str) -> bool:
        self.spoken.append(text)
        return self.complete
```

- [ ] **Step 5: Run the full suite, then commit**

Run: `python3 -m pytest -q` - Expected: all green (the return value is new, nothing asserts on it yet).

```bash
git add src/sonari/speaker.py tests/test_speaker.py tests/daemon_helpers.py
git commit -m "feat(speaker): speak() reports completion - the sentence-granular heard signal"
```

---

### Task 2: The history substrate (`src/sonari/history.py`, pure, TDD)

**Files:**
- Create: `src/sonari/history.py`
- Create: `tests/test_history.py`

- [ ] **Step 1: Write the failing tests** - create `tests/test_history.py`:

```python
from sonari.history import SessionHistory


def test_record_and_last_message_groups_by_message_boundary():
    h = SessionHistory()
    h.record("s1", "prose", "First sentence.")
    h.record("s1", "prose", "Second sentence.")
    h.end_message("s1")
    h.record("s1", "prose", "Next message.")
    assert [e.text for e in h.last_message("s1")] == ["Next message."]


def test_last_message_returns_whole_group():
    h = SessionHistory()
    h.record("s1", "prose", "A.")
    h.record("s1", "prose", "B.")
    h.end_message("s1")
    assert [e.text for e in h.last_message("s1")] == ["A.", "B."]


def test_last_message_empty_session():
    h = SessionHistory()
    assert h.last_message("nope") == []


def test_unheard_until_marked():
    h = SessionHistory()
    e1 = h.record("s1", "prose", "A.")
    e2 = h.record("s1", "prose", "B.")
    assert [e.text for e in h.unheard("s1")] == ["A.", "B."]
    e1.heard = True
    assert [e.text for e in h.unheard("s1")] == ["B."]
    e2.heard = True
    assert h.unheard("s1") == []


def test_reset_drops_session():
    h = SessionHistory()
    h.record("s1", "prose", "A.")
    h.reset("s1")
    assert h.last_message("s1") == []
    assert h.unheard("s1") == []


def test_rolling_cap_bounds_memory():
    h = SessionHistory(cap=3)
    for i in range(10):
        h.record("s1", "prose", "S{0}.".format(i))
    texts = [e.text for e in h.unheard("s1")]
    assert texts == ["S7.", "S8.", "S9."]


def test_other_session_with_unheard_most_recent_first():
    h = SessionHistory()
    h.record("a", "prose", "A1.")
    h.record("b", "prose", "B1.")          # b touched most recently
    assert h.other_session_with_unheard("fg") == "b"
    for e in h.unheard("b"):
        e.heard = True
    assert h.other_session_with_unheard("fg") == "a"
    for e in h.unheard("a"):
        e.heard = True
    assert h.other_session_with_unheard("fg") is None


def test_other_session_excludes_the_given_session():
    h = SessionHistory()
    h.record("fg", "prose", "X.")
    assert h.other_session_with_unheard("fg") is None
```

- [ ] **Step 2: Run to verify they fail**

Run: `python3 -m pytest tests/test_history.py -q`
Expected: FAIL - `ModuleNotFoundError: sonari.history`.

- [ ] **Step 3: Implement** - create `src/sonari/history.py`:

```python
"""Per-session narration history + sentence-granular heard-marker.

PURE: no I/O. The Phase 2.1 substrate behind repeat / catch_up /
voice-continuity capture: every narrated-or-captured sentence is recorded per
session; `heard` flips True only when the speak loop confirms the utterance
COMPLETED, so an interrupted sentence stays unheard and a replay restarts from
the start of that sentence.
"""
from __future__ import annotations

from collections import deque


class HistoryEntry:
    __slots__ = ("text", "kind", "msg_id", "heard")

    def __init__(self, text: str, kind: str, msg_id: int) -> None:
        self.text = text
        self.kind = kind          # prose|choice|plan|permission
        self.msg_id = msg_id      # message group; bumped by end_message()
        self.heard = False


class SessionHistory:
    def __init__(self, cap: int = 200) -> None:
        self._cap = cap
        self._entries: "dict[str, deque]" = {}
        self._msg_id: "dict[str, int]" = {}
        self._touch: "dict[str, int]" = {}   # recency across sessions
        self._tick = 0

    def record(self, session: str, kind: str, text: str) -> HistoryEntry:
        d = self._entries.get(session)
        if d is None:
            d = deque(maxlen=self._cap)
            self._entries[session] = d
        entry = HistoryEntry(text, kind, self._msg_id.get(session, 0))
        d.append(entry)
        self._tick += 1
        self._touch[session] = self._tick
        return entry

    def end_message(self, session: str) -> None:
        """Close the current message group (the assembler's final boundary)."""
        self._msg_id[session] = self._msg_id.get(session, 0) + 1

    def last_message(self, session: str) -> list:
        """All entries of the most recent message group (the 'whole last
        message'), oldest first."""
        d = self._entries.get(session)
        if not d:
            return []
        last_id = d[-1].msg_id
        return [e for e in d if e.msg_id == last_id]

    def unheard(self, session: str) -> list:
        """All not-yet-completed entries for session, oldest first."""
        return [e for e in self._entries.get(session, ()) if not e.heard]

    def reset(self, session: str) -> None:
        """Forget a session entirely (new prompt / session end)."""
        self._entries.pop(session, None)
        self._msg_id.pop(session, None)
        self._touch.pop(session, None)

    def other_session_with_unheard(self, exclude: str):
        """The most recently active OTHER session that has unheard entries,
        or None. Lets catch_up recover a session you left without re-typing
        in it (there is no OS window-focus hook)."""
        best, best_tick = None, -1
        for session, tick in self._touch.items():
            if session == exclude:
                continue
            if tick > best_tick and self.unheard(session):
                best, best_tick = session, tick
        return best
```

- [ ] **Step 4: Run to verify they pass**

Run: `python3 -m pytest tests/test_history.py -q` - Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/history.py tests/test_history.py
git commit -m "feat(history): per-session rolling narration history + heard-marker (pure substrate)"
```

---

### Task 3: Daemon records history; speak loop confirms "heard"

**Files:**
- Modify: `src/sonari/daemon.py` (`__init__`, `_enqueue`, PROSE branch, `_speak_loop`)
- Modify: `src/sonari/queue.py` (`clear`/`flush_session` return dropped items)
- Modify: `src/sonari/config.py` (DEFAULTS + `"history_cap": 200`)
- Test: `tests/test_daemon_phase21.py` (new), `tests/test_queue.py` (append)

- [ ] **Step 1: Failing tests** - create `tests/test_daemon_phase21.py`:

```python
from sonari.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


def _prose(daemon, session, text, index=0, final=True):
    daemon.handle_message(_msg(MsgType.PROSE, session, delta=text, index=index,
                               final=final))


def _drain_one(daemon, queue, speaker):
    """Pop one queued item and run it through the speak-loop bookkeeping."""
    item = queue.pop_next()
    assert item is not None
    completed = speaker.speak(item.text)
    daemon.note_spoken(item, completed)
    return item


# --- recording -------------------------------------------------------------

def test_prose_chunks_recorded_per_session():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "One. Two. ")
    assert [e.text for e in daemon.history.unheard("fg")] == ["One.", "Two."]


def test_final_closes_the_message_group():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "First. ")
    _prose(daemon, "fg", "Second. ")
    assert [e.text for e in daemon.history.last_message("fg")] == ["Second."]


# --- heard marking ----------------------------------------------------------

def test_completed_speech_marks_heard():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Hello there. ")
    _drain_one(daemon, queue, speaker)
    assert daemon.history.unheard("fg") == []


def test_interrupted_sentence_stays_unheard():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Hello there. ")
    speaker.complete = False                      # simulate cancel mid-sentence
    _drain_one(daemon, queue, speaker)
    assert [e.text for e in daemon.history.unheard("fg")] == ["Hello there."]


def test_stop_leaves_entries_unheard():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "A. B. ")
    daemon.handle_message(_msg(MsgType.STOP))
    assert len(queue) == 0
    assert [e.text for e in daemon.history.unheard("fg")] == ["A.", "B."]


def test_user_prompt_flush_resets_history():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Old stuff. ")
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    assert daemon.history.unheard("fg") == []
    assert daemon.history.last_message("fg") == []


def test_history_cap_comes_from_config():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon.history._cap == config["history_cap"] == 200
```

And append to `tests/test_queue.py`:

```python
def test_clear_returns_dropped_items():
    from sonari.queue import SpeechQueue, SpeechItem
    q = SpeechQueue()
    q.enqueue(SpeechItem(1, "s", "prose", "A.", False))
    q.enqueue(SpeechItem(2, "s", "prose", "B.", False))
    dropped = q.clear()
    assert [i.id for i in dropped] == [1, 2]
    assert len(q) == 0


def test_flush_session_returns_dropped_items():
    from sonari.queue import SpeechQueue, SpeechItem
    q = SpeechQueue()
    q.enqueue(SpeechItem(1, "a", "prose", "A.", False))
    q.enqueue(SpeechItem(2, "b", "prose", "B.", False))
    dropped = q.flush_session("a")
    assert [i.id for i in dropped] == [1]
    assert len(q) == 1
```

- [ ] **Step 2: Run to verify failures**

Run: `python3 -m pytest tests/test_daemon_phase21.py tests/test_queue.py -q`
Expected: FAIL - no `daemon.history`, no `note_spoken`, `clear()` returns None.

- [ ] **Step 3: Implement `queue.py`** - replace `clear` and `flush_session`:

```python
    def clear(self) -> "list[SpeechItem]":
        dropped = list(self._items)
        self._items.clear()
        return dropped

    def flush_session(self, session: str) -> "list[SpeechItem]":
        dropped = [i for i in self._items if i.session == session]
        self._items = deque(
            item for item in self._items if item.session != session
        )
        return dropped
```

- [ ] **Step 4: Implement `config.py`** - add to `DEFAULTS` (after `"background_policy"`):

```python
    "history_cap": 200,
```

- [ ] **Step 5: Implement `daemon.py`.**

In `SpeechDaemon.__init__`, replace `self._last_spoken: str | None = None` and `self._last_options: str | None = None` with:

```python
        from sonari.history import SessionHistory
        self.history = SessionHistory(cap=int(config.get("history_cap", 200)))
        self._options: "dict[str, str]" = {}
        self._voice_owner: "str | None" = None
        self._captured_msg: "set[str]" = set()
        self._pending_heard: dict = {}            # SpeechItem.id -> HistoryEntry
        self._current_item = None                 # item being spoken right now
```

(Keep the `_last_options` *uses* compiling for now by replacing them per-branch in this and later tasks - this task touches PROSE/STOP/FLUSH/SESSION_END; Task 5/6/7 rewrite REPEAT/CATCH_UP/REREAD and CHOICE/PLAN/PERMISSION. To keep every intermediate commit green, in THIS task replace the simple `self._last_options = None` statements in FLUSH/SESSION_END with `self._options.pop(session, None)` and leave the CHOICE/PLAN/PERMISSION branches setting `self._options[session] = text` instead of `self._last_options = text`; update REREAD to read `self._options.get(self.sessions.foreground())` - behaviorally identical for a single session.)

New `_enqueue` (entry correlation) - replace the existing method:

```python
    def _enqueue(self, session: str, kind: str, text: str, is_decision: bool,
                 entry=None) -> None:
        item = SpeechItem(
            id=self._alloc_id(),
            session=session,
            kind=kind,
            text=text,
            is_decision=is_decision,
        )
        if entry is not None:
            self._pending_heard[item.id] = entry
        self.queue.enqueue(item)
        self._wake.set()
```

New helper + speak-loop bookkeeping (used by tests and `_speak_loop`):

```python
    def _drop_pending(self, items) -> None:
        for it in items:
            self._pending_heard.pop(it.id, None)

    def note_spoken(self, item, completed: bool) -> None:
        """Speak-loop bookkeeping: confirm (or decline) the heard-marker for a
        finished utterance, and release the voice when the queue drains."""
        with self._lock:
            self._current_item = None
            entry = self._pending_heard.pop(item.id, None)
            if entry is not None and completed:
                entry.heard = True
            if len(self.queue) == 0:
                self._voice_owner = None
```

New `_speak_loop` - replace the existing one:

```python
    def _speak_loop(self) -> None:
        self._running.set()
        while self._running.is_set():
            item = self.queue.pop_next()
            if item is not None:
                with self._lock:
                    self._current_item = item
                completed = self.speaker.speak(item.text)
                self.note_spoken(item, completed)
                continue
            with self._lock:
                if self._voice_owner is not None and len(self.queue) == 0:
                    self._voice_owner = None
            # nothing to say: wait until woken by an enqueue or until stop()
            self._wake.wait(self._poll_interval)
            self._wake.clear()
```

New PROSE branch - replace the existing one (voice gating becomes real in Task 4; this task introduces `_may_speak` already):

```python
        if t == MsgType.PROSE:
            a = self._assembler(session)
            chunks = a.feed(msg.get("delta", ""), msg.get("index", 0), msg.get("final", False))
            if chunks:
                speak = verbosity != "quiet" and self._may_speak(session)
                for chunk in chunks:
                    entry = self.history.record(session, "prose", chunk)
                    if speak:
                        self._enqueue(session, "prose", chunk, False, entry=entry)
                    else:
                        self._captured_msg.add(session)
            if msg.get("final", False):
                self.history.end_message(session)
                self._captured_msg.discard(session)
            return None
```

`_may_speak` (the voice-continuity core, fully exercised in Task 4):

```python
    def _may_speak(self, session: str) -> bool:
        """Voice continuity: a busy voice stays with its owner to the end; a
        free voice is acquired only by the FOREGROUND session, and only at a
        message boundary (a message that started captured stays captured)."""
        if self._voice_owner == session:
            return True
        if (self._voice_owner is None
                and self.sessions.is_foreground(session)
                and session not in self._captured_msg):
            self._voice_owner = session
            return True
        return False
```

STOP branch - replace:

```python
        if t == MsgType.STOP:
            self._drop_pending(self.queue.clear())
            self.speaker.cancel()
            self._voice_owner = None
            return None
```

(The cancelled utterance's pending entry is popped by `note_spoken` with `completed=False` → stays unheard. Dropped queue items were recorded at handle time and never marked → stay unheard. Both recoverable by catch_up, per spec.)

FLUSH branch - replace (cancel **only our own** current utterance - voice continuity):

```python
        if t == MsgType.FLUSH:
            self._drop_pending(self.queue.flush_session(session))
            cur = self._current_item
            if cur is not None and cur.session == session:
                self.speaker.cancel()
            if self._voice_owner == session:
                self._voice_owner = None
            self._assemblers.pop(session, None)
            self.history.reset(session)
            self._captured_msg.discard(session)
            self._options.pop(session, None)
            return None
```

SESSION_END branch - replace the cleanup lines:

```python
        if t == MsgType.SESSION_END:
            self.sessions.unregister(session)
            self._drop_pending(self.queue.flush_session(session))
            if self._voice_owner == session:
                self._voice_owner = None
            self.history.reset(session)
            self._captured_msg.discard(session)
            self._options.pop(session, None)
            self._warned_immediate.discard(session)
            self._guided_sessions.discard(session)
            return None
```

CHOICE/PLAN/PERMISSION branches: change only `self._last_options = text` → `self._options[session] = text` (full rewrite comes in Task 6/7). REPEAT branch: temporarily keep compiling by replacing its body with the Task-5 implementation now if trivial, otherwise leave `_last_spoken`-free minimal stub:

```python
        if t == MsgType.REPEAT:
            fg = self.sessions.foreground()
            if fg is None:
                return None
            entries = self.history.last_message(fg)
            for e in entries:
                self._enqueue(fg, "prose", e.text, False, entry=e)
            return None
```

(Task 5 finishes REPEAT with the "Nothing to repeat." path + its tests.)

- [ ] **Step 6: Fix legacy tests that encoded the old semantics** (the spec deliberately changes them):

In `tests/test_daemon_control.py`:
- `test_flush_drops_session_items_and_cancels` - flush now cancels only when the **current utterance belongs to the flushed session**; there is no current utterance in the unit test, so assert `speaker.cancels == 0` and rename to `test_flush_drops_session_items_without_cancelling_other_speech`.
- `test_repeat_noop_when_nothing_spoken_yet` / `test_repeat_reenqueues_last_spoken_text` / `test_repeat_drives_speak_path` - repeat is now history-based; update to enqueue prose via `handle_message` + drain with `_drain_one`-style bookkeeping, then assert the **whole message** re-enqueues (Task 5 finalizes; minimally update here to keep green).

In `tests/test_daemon_phase2.py`: `test_flush_clears_option_cache` - options are per-session now; the assertion stands but goes through `daemon._options` absence → keep the message-level behavior assert (reread after flush speaks "No options to repeat." until Task 7 changes the wording).

- [ ] **Step 7: Run the FULL suite; fix fallout until green**

Run: `python3 -m pytest -q`
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add src/sonari/daemon.py src/sonari/queue.py src/sonari/config.py tests/
git commit -m "feat(daemon): record narration history per session; speak loop confirms sentence-level heard"
```

---

### Task 4: Voice continuity + silent capture (multi-session)

**Files:**
- Modify: `src/sonari/daemon.py` (CHOICE/PLAN/PERMISSION/TOOL gating via `_may_speak`)
- Test: `tests/test_daemon_phase21.py` (append)

- [ ] **Step 1: Failing tests** (append to `tests/test_daemon_phase21.py`):

```python
# --- voice continuity / capture ---------------------------------------------

def test_foreground_session_acquires_free_voice():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "Hi. ", final=False)
    assert daemon._voice_owner == "a"
    assert len(queue) == 1


def test_nonforeground_response_is_captured_not_spoken():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "b", "Background. ")
    assert len(queue) == 0                                  # not spoken live
    assert [e.text for e in daemon.history.unheard("b")] == ["Background."]


def test_owner_keeps_voice_after_foreground_moves():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A speaking. ", final=False)        # a owns the voice
    sessions.set_foreground("b")                            # user prompts in b
    _prose(daemon, "a", "Still a. ", index=1, final=False)  # a keeps talking
    assert daemon._voice_owner == "a"
    assert len(queue) == 2


def test_response_landing_on_busy_voice_stays_captured_to_its_end():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A holds the voice. ", final=False)
    sessions.set_foreground("b")
    _prose(daemon, "b", "B part one. ", final=False)        # voice busy -> captured
    # a drains; voice frees
    while len(queue):
        _drain_one(daemon, queue, speaker)
    # b's SAME message continues -> still captured (no mid-thought join)
    _prose(daemon, "b", "B part two. ", index=1, final=True)
    assert len(queue) == 0
    texts = [e.text for e in daemon.history.unheard("b")]
    assert texts == ["B part one.", "B part two."]
    # b's NEXT message may acquire the free voice (b is foreground)
    _prose(daemon, "b", "B fresh message. ")
    assert daemon._voice_owner == "b"
    assert len(queue) == 1


def test_voice_frees_but_never_autostarts_nonforeground_backlog():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "b", "B backlog. ")                      # captured
    assert daemon._voice_owner is None
    assert len(queue) == 0                                  # stays silent


def test_choice_for_nonowner_is_captured_and_options_stored():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A talking. ", final=False)
    sessions.set_foreground("b")
    daemon.handle_message(_msg(MsgType.CHOICE, "b", questions=[
        {"question": "Pick one?", "options": [{"label": "X"}, {"label": "Y"}]}
    ]))
    assert len(queue) == 1                                  # only a's prose queued
    assert "Pick one?" in daemon._options["b"]              # reread works on return
    assert daemon.history.unheard("b")                      # captured for catch_up
```

- [ ] **Step 2: Run to verify failures**

Run: `python3 -m pytest tests/test_daemon_phase21.py -q`
Expected: the new tests FAIL (CHOICE still gates on `should_speak`, etc.).

- [ ] **Step 3: Implement** - in `daemon.py`, rewrite the CHOICE/PLAN/PERMISSION/TOOL branches to record + capture (this also completes the Task-3 transitional edits):

```python
        if t == MsgType.CHOICE:
            text = self._choice_text(msg)
            extras = [e for e in (
                self._choice_notes(msg),
                self._selection_cue(session, verbosity),
            ) if e]
            if extras:
                text = "{0} {1}".format(text, " ".join(extras))
            self._options[session] = text
            entry = self.history.record(session, "choice", text)
            self.history.end_message(session)
            if self._may_speak(session):
                self._enqueue(session, "choice", text, True, entry=entry)
            return None

        if t == MsgType.PLAN:
            text = self._plan_text(msg)
            cue = self._selection_cue(session, verbosity)
            if cue:
                text = "{0} {1}".format(text, cue)
            self._options[session] = text
            entry = self.history.record(session, "plan", text)
            self.history.end_message(session)
            if self._may_speak(session):
                self._enqueue(session, "plan", text, True, entry=entry)
            return None

        if t == MsgType.PERMISSION:
            text = self._permission_text(msg)
            cue = self._selection_cue(session, verbosity)
            if cue:
                text = "{0} {1}".format(text, cue)
            self._options[session] = text
            entry = self.history.record(session, "permission", text)
            self.history.end_message(session)
            if self._may_speak(session):
                self._enqueue(session, "permission", text, True, entry=entry)
            return None

        if t == MsgType.TOOL:
            if verbosity == "everything" and self._may_speak(session):
                tool = msg.get("tool", "")
                summary = (msg.get("summary") or "").strip()
                text = summary if summary else "Running {0}.".format(tool)
                self._enqueue(session, "tool_announce", text, False)
            return None
```

(Note: decision prompts are recorded/stored **even for non-speaking sessions** - that's what makes reread-on-return and catch_up work. `tool_announce` is deliberately NOT recorded.)

SKIP branch - replace (deliberate skip = heard):

```python
        if t == MsgType.SKIP:
            cur = self._current_item
            if cur is not None:
                entry = self._pending_heard.get(cur.id)
                if entry is not None:
                    entry.heard = True
            self.speaker.cancel()
            return None
```

- [ ] **Step 4: Run the full suite; update any `should_speak`-era daemon tests** (`tests/test_daemon_prose.py`, `tests/test_daemon_decisions.py` gate non-foreground sessions - their *observable* behavior is unchanged: non-foreground content doesn't enqueue; only internals moved to capture. Expect at most assertion-message tweaks.)

Run: `python3 -m pytest -q` - Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/daemon.py tests/
git commit -m "feat(daemon): voice continuity - owner keeps the voice; non-owner content captured silently per session"
```

---

### Task 5: `repeat` re-speaks the entire last message

**Files:**
- Modify: `src/sonari/daemon.py` (REPEAT branch)
- Test: `tests/test_daemon_phase21.py` (append); finalize `tests/test_daemon_control.py` repeat tests

- [ ] **Step 1: Failing tests** (append):

```python
# --- repeat ------------------------------------------------------------------

def test_repeat_respeaks_whole_last_message_not_last_fragment():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "First sentence. Second sentence. Third. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REPEAT))
    texts = []
    while len(queue):
        texts.append(queue.pop_next().text)
    assert texts == ["First sentence.", "Second sentence.", "Third."]


def test_repeat_targets_last_message_only():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Old message. ")
    _prose(daemon, "fg", "New message. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REPEAT))
    item = queue.pop_next()
    assert item.text == "New message."
    assert len(queue) == 0


def test_repeat_with_no_history_says_nothing_to_repeat():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.REPEAT))
    item = queue.pop_next()
    assert item.text == "Nothing to repeat."


def test_repeat_acts_on_foreground_session_history():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A message. ")
    _prose(daemon, "b", "B captured. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REPEAT))
    item = queue.pop_next()
    assert item.text == "A message."
```

- [ ] **Step 2: Run to verify the new tests fail** (`"Nothing to repeat."` missing).

Run: `python3 -m pytest tests/test_daemon_phase21.py -q -k repeat`

- [ ] **Step 3: Implement** - final REPEAT branch:

```python
        if t == MsgType.REPEAT:
            fg = self.sessions.foreground()
            if fg is None:
                return None
            entries = self.history.last_message(fg)
            if not entries:
                self._enqueue(fg, "prose", "Nothing to repeat.", False)
                return None
            for e in entries:
                self._enqueue(fg, e.kind, e.text, False, entry=e)
            return None
```

- [ ] **Step 4: Finalize the legacy repeat tests in `tests/test_daemon_control.py`** to the new semantics (whole-message; "Nothing to repeat." replaces silent no-op when foreground exists; still a silent no-op with no foreground).

- [ ] **Step 5: Full suite → green, commit**

```bash
python3 -m pytest -q
git add src/sonari/daemon.py tests/
git commit -m "feat(daemon): repeat re-speaks the entire last message from history"
```

---

### Task 6: `catch_up` replays the unheard backlog (and crosses sessions)

**Files:**
- Modify: `src/sonari/daemon.py` (CATCH_UP branch)
- Test: `tests/test_daemon_phase21.py` (append); rewrite `tests/test_daemon_control.py::test_catch_up_clears_and_cancels`

- [ ] **Step 1: Failing tests** (append):

```python
# --- catch_up ----------------------------------------------------------------

def test_catch_up_replays_unheard_oldest_first_then_marks_heard():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "One. Two. ")
    daemon.handle_message(_msg(MsgType.STOP))               # heard nothing
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = []
    while len(queue):
        texts.append(_drain_one(daemon, queue, speaker).text)
    assert texts == ["One.", "Two."]
    assert daemon.history.unheard("fg") == []               # marker advanced


def test_catch_up_interrupted_sentence_replays_from_its_start():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Long sentence here. ")
    speaker.complete = False                                # cut mid-sentence
    _drain_one(daemon, queue, speaker)
    speaker.complete = True
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    item = queue.pop_next()
    assert item.text == "Long sentence here."               # from the start


def test_catch_up_all_heard_says_caught_up():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Hi. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    item = queue.pop_next()
    assert item.text == "You're all caught up."


def test_catch_up_falls_back_to_other_session_backlog():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A heard. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
    _prose(daemon, "b", "B unheard. ")                      # captured silently
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = []
    while len(queue):
        texts.append(queue.pop_next().text)
    assert texts == ["Catching up on another session.", "B unheard."]


def test_catch_up_does_not_double_speak_queued_items():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Queued one. Queued two. ")        # in queue, unheard
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = []
    while len(queue):
        texts.append(queue.pop_next().text)
    assert texts == ["Queued one.", "Queued two."]          # once, not twice
```

- [ ] **Step 2: Run to verify failures** (old catch_up clears and speaks nothing).

Run: `python3 -m pytest tests/test_daemon_phase21.py -q -k catch_up`

- [ ] **Step 3: Implement** - final CATCH_UP branch (replaces the clear-and-cancel body):

```python
        if t == MsgType.CATCH_UP:
            fg = self.sessions.foreground()
            if fg is None:
                return None
            target = fg
            entries = self.history.unheard(fg)
            preamble = None
            if not entries:
                other = self.history.other_session_with_unheard(fg)
                if other is not None:
                    target = other
                    entries = self.history.unheard(other)
                    preamble = "Catching up on another session."
            if not entries:
                self._enqueue(fg, "prose", "You're all caught up.", False)
                return None
            # Replay cleanly: cut the target's current utterance (it stays
            # unheard, so it replays FROM ITS START) and drop its queued
            # duplicates - every unheard entry is re-enqueued in order below.
            cur = self._current_item
            if cur is not None and cur.session == target:
                self.speaker.cancel()
            self._drop_pending(self.queue.flush_session(target))
            if preamble:
                self._enqueue(fg, "prose", preamble, False)
            for e in entries:
                self._enqueue(target, e.kind, e.text,
                              e.kind in ("choice", "plan", "permission"),
                              entry=e)
            return None
```

- [ ] **Step 4: Rewrite `tests/test_daemon_control.py::test_catch_up_clears_and_cancels`** - the old "clears everything" contract is gone by design; replace it with a pointer test:

```python
def test_catch_up_no_longer_discards_the_backlog():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PROSE, "fg", delta="Keep me. ",
                               index=0, final=True))
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = [queue.pop_next().text for _ in range(len(queue))]
    assert "Keep me." in texts
```

- [ ] **Step 5: Full suite → green, commit**

```bash
python3 -m pytest -q
git add src/sonari/daemon.py tests/
git commit -m "feat(daemon): catch_up replays the unheard backlog from the marker (sentence-start resume, cross-session fallback)"
```

---

### Task 7: `reread_options` - dedicated per-session slot, descriptions, multi-select announce

**Files:**
- Modify: `src/sonari/daemon.py` (`_choice_text`, REREAD_OPTIONS branch)
- Test: `tests/test_daemon_phase21.py` (append); update `tests/test_daemon_phase2.py` reread tests + `tests/test_daemon_decisions.py` choice-text tests

- [ ] **Step 1: Failing tests** (append):

```python
# --- reread_options ----------------------------------------------------------

def _choice(daemon, session, questions):
    daemon.handle_message(_msg(MsgType.CHOICE, session, questions=questions))


def test_choice_speaks_descriptions_and_numbers():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{
        "question": "Auth method?",
        "options": [
            {"label": "OAuth", "description": "Use Google sign-in"},
            {"label": "Magic link"},
        ],
    }])
    item = queue.pop_next()
    assert "Option 1: OAuth. Use Google sign-in." in item.text
    assert "Option 2: Magic link." in item.text


def test_multiselect_announced_up_front():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{
        "question": "Pick features.",
        "multiSelect": True,
        "options": [{"label": "A"}, {"label": "B"}],
    }])
    item = queue.pop_next()
    assert "This is a multi-select; you can pick more than one." in item.text


def test_reread_speaks_current_options_not_queue_tail():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{"question": "Q?", "options": [{"label": "X"}]}])
    while len(queue):
        _drain_one(daemon, queue, speaker)
    _prose(daemon, "fg", "Other speech happened after. ")   # queue tail moves on
    while len(queue):
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))
    item = queue.pop_next()
    assert "Option 1: X." in item.text                       # the options, not the tail


def test_reread_with_no_active_options_says_so():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))
    item = queue.pop_next()
    assert item.text == "No options right now."


def test_reread_after_flush_says_no_options():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{"question": "Q?", "options": [{"label": "X"}]}])
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))
    item = queue.pop_next()
    assert item.text == "No options right now."


def test_reread_is_per_session():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _choice(daemon, "b", [{"question": "B q?", "options": [{"label": "BB"}]}])
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))      # fg=a has none
    item = queue.pop_next()
    assert item.text == "No options right now."
```

- [ ] **Step 2: Run to verify failures.**

Run: `python3 -m pytest tests/test_daemon_phase21.py -q -k "reread or choice or multiselect"`

- [ ] **Step 3: Implement.** Replace `_choice_text` (descriptions + mode announce; numbering follows the RAW option index so spoken numbers always match the TUI's digit keys):

```python
    @staticmethod
    def _choice_text(msg) -> str:
        parts = []
        for q in msg.get("questions", []) or []:
            qtext = q.get("question", "") if isinstance(q, dict) else str(q)
            multi = bool(isinstance(q, dict) and q.get("multiSelect"))
            opts = q.get("options", []) if isinstance(q, dict) else []
            segs = []
            for i, o in enumerate(opts, 1):
                if isinstance(o, dict):
                    label = o.get("label", "")
                    desc = (o.get("description") or "").strip()
                else:
                    label, desc = str(o), ""
                if not label:
                    continue   # keep numbering aligned with the TUI's digits
                seg = "Option {0}: {1}.".format(i, label)
                if desc:
                    seg += " {0}{1}".format(
                        desc, "" if desc.endswith((".", "!", "?")) else ".")
                segs.append(seg)
            head = qtext
            if multi:
                head = "{0}{1}".format(
                    (qtext + " ") if qtext else "",
                    "This is a multi-select; you can pick more than one.")
            if head and segs:
                parts.append("{0} {1}".format(head, " ".join(segs)))
            elif segs:
                parts.append(" ".join(segs))
            elif head:
                parts.append(head)
        return " ".join(parts) if parts else "A question needs your answer."
```

Replace the REREAD_OPTIONS branch:

```python
        if t == MsgType.REREAD_OPTIONS:
            fg = self.sessions.foreground()
            if fg is None:
                return None
            text = self._options.get(fg)
            if text:
                self._enqueue(fg, "choice", text, False)
            else:
                self._enqueue(fg, "prose", "No options right now.", False)
            return None
```

Close the options slot when the prompt is answered (prose resumes for that session) - add at the END of the PROSE branch's `if msg.get("final", False):` block:

```python
                self._options.pop(session, None)
```

(Options are stored at CHOICE/PLAN/PERMISSION time; the next *completed* prose message from that session means the prompt was dealt with → reread honestly reports "No options right now.")

- [ ] **Step 4: Update legacy expectations**: `tests/test_daemon_phase2.py` reread tests ("No options to repeat." → "No options right now."; option-cache asserts move to `daemon._options`) and any `tests/test_daemon_decisions.py` `_choice_text` literals (now include descriptions).

- [ ] **Step 5: Full suite → green, commit**

```bash
python3 -m pytest -q
git add src/sonari/daemon.py tests/
git commit -m "feat(daemon): reread_options reads a per-session live-options slot (descriptions, multi-select announce, honest empty state)"
```

---

### Task 8: Caret tracking - daemon side (mirror cursor + virtual Submit + prompt-open flag)

**Files:**
- Modify: `src/sonari/protocol.py` (new MsgType)
- Modify: `src/sonari/paths.py` (PROMPT_OPEN_PATH)
- Modify: `src/sonari/daemon.py` (caret state, CARET_MOVE branch, flag lifecycle)
- Test: `tests/test_daemon_phase21.py` (append)

- [ ] **Step 1: Failing tests** (append):

```python
# --- caret tracking ----------------------------------------------------------

def _open_multiselect(daemon):
    _choice(daemon, "fg", [{
        "question": "Pick.",
        "multiSelect": True,
        "options": [{"label": "Alpha"}, {"label": "Beta"}],
    }])


def test_caret_speaks_focused_option_on_move():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _open_multiselect(daemon)
    while len(queue):
        queue.pop_next()
    daemon.handle_message(_msg("caret_move", dir="down"))
    item = queue.pop_next()
    assert item.text == "Option 2: Beta."


def test_caret_past_last_option_is_submit_on_multiselect():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _open_multiselect(daemon)
    while len(queue):
        queue.pop_next()
    daemon.handle_message(_msg("caret_move", dir="down"))     # -> Beta
    daemon.handle_message(_msg("caret_move", dir="down"))     # -> Submit
    texts = [queue.pop_next().text for _ in range(len(queue))]
    assert texts[-1] == "Submit."


def test_caret_clamps_at_edges():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _open_multiselect(daemon)
    while len(queue):
        queue.pop_next()
    daemon.handle_message(_msg("caret_move", dir="up"))        # clamp at top
    item = queue.pop_next()
    assert item.text == "Option 1: Alpha."


def test_caret_single_select_has_no_submit_row():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{
        "question": "One?",
        "options": [{"label": "Only"}],
    }])
    while len(queue):
        queue.pop_next()
    daemon.handle_message(_msg("caret_move", dir="down"))      # clamp: no Submit
    item = queue.pop_next()
    assert item.text == "Option 1: Only."


def test_caret_inert_without_open_prompt():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg("caret_move", dir="down"))
    assert len(queue) == 0


def test_caret_resets_on_each_new_prompt():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _open_multiselect(daemon)
    daemon.handle_message(_msg("caret_move", dir="down"))      # pos -> 1
    _open_multiselect(daemon)                                  # fresh prompt
    while len(queue):
        queue.pop_next()
    daemon.handle_message(_msg("caret_move", dir="down"))
    item = queue.pop_next()
    assert item.text == "Option 2: Beta."                      # snapped back to top first


def test_caret_closes_when_prompt_answered():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _open_multiselect(daemon)
    _prose(daemon, "fg", "Answered; moving on. ")
    while len(queue):
        queue.pop_next()
    daemon.handle_message(_msg("caret_move", dir="down"))
    assert len(queue) == 0


def test_caret_inert_for_multi_question_prompts():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [
        {"question": "Q1?", "options": [{"label": "A"}]},
        {"question": "Q2?", "options": [{"label": "B"}]},
    ])
    while len(queue):
        queue.pop_next()
    daemon.handle_message(_msg("caret_move", dir="down"))
    assert len(queue) == 0
```

- [ ] **Step 2: Run to verify failures.**

Run: `python3 -m pytest tests/test_daemon_phase21.py -q -k caret`

- [ ] **Step 3: Implement.**

`protocol.py` - add to `MsgType`:

```python
    CARET_MOVE = "caret_move"
```

`paths.py` - add after `INSTALL_RECORD_PATH`:

```python
PROMPT_OPEN_PATH = SONARI_DIR / "prompt-open"   # exists IFF a caret-trackable prompt is open
```

`daemon.py` - import `PROMPT_OPEN_PATH` in the existing `from sonari.paths import (...)`; add to `__init__`:

```python
        self._caret = None   # {"session","labels","submit","pos"} while a prompt is open
```

Flag + caret lifecycle helpers (new methods):

```python
    def _open_caret(self, session: str, questions) -> None:
        """Arm caret tracking for a single-question prompt; otherwise disarm.
        The flag file tells hotkeyd to forward arrow keys (listen-only)."""
        self._close_caret()
        if len(questions) != 1 or not isinstance(questions[0], dict):
            return
        q = questions[0]
        labels = []
        for o in q.get("options", []) or []:
            label = o.get("label", "") if isinstance(o, dict) else str(o)
            if label:
                labels.append(label)
        if not labels:
            return
        self._caret = {
            "session": session,
            "labels": labels,
            "submit": bool(q.get("multiSelect")),
            "pos": 0,   # the TUI opens with the first option highlighted
        }
        try:
            PROMPT_OPEN_PATH.touch()
        except OSError:
            pass   # caret narration degrades; never break the daemon

    def _close_caret(self) -> None:
        self._caret = None
        try:
            os.unlink(str(PROMPT_OPEN_PATH))
        except FileNotFoundError:
            pass
        except OSError:
            pass
```

Wire the lifecycle: in the CHOICE branch (Task 4 version), after `self._options[session] = text` add `self._open_caret(session, msg.get("questions", []) or [])`. In the PROSE branch's `final` block, after `self._options.pop(session, None)` add:

```python
                if self._caret is not None and self._caret["session"] == session:
                    self._close_caret()
```

In FLUSH and SESSION_END, after `self._options.pop(session, None)` add the same two-line close. PLAN/PERMISSION branches call `self._close_caret()` (they replace any open choice prompt and have no option list).

CARET_MOVE branch (add near REREAD_OPTIONS):

```python
        if t == MsgType.CARET_MOVE:
            c = self._caret
            fg = self.sessions.foreground()
            if c is None or fg is None or c["session"] != fg:
                return None
            last = len(c["labels"]) - 1 + (1 if c["submit"] else 0)
            step = 1 if msg.get("dir") == "down" else -1
            c["pos"] = max(0, min(last, c["pos"] + step))
            if c["submit"] and c["pos"] == len(c["labels"]):
                spoken = "Submit."
            else:
                spoken = "Option {0}: {1}.".format(c["pos"] + 1,
                                                   c["labels"][c["pos"]])
            self.speaker.cancel()   # snappy: cut the previous announcement
            self._enqueue(fg, "prose", spoken, False)
            return None
```

(Caret announcements are NOT recorded in history - they're navigation echo, not content. `_enqueue` without `entry=` does exactly that.)

Also ensure tests never touch the real `~/.sonari`: in `tests/test_daemon_phase21.py` add a module-level autouse fixture:

```python
import pytest


@pytest.fixture(autouse=True)
def _tmp_prompt_flag(tmp_path, monkeypatch):
    import sonari.daemon as daemon_mod
    monkeypatch.setattr(daemon_mod, "PROMPT_OPEN_PATH",
                        tmp_path / "prompt-open")
```

- [ ] **Step 4: Run the full suite → green, commit**

```bash
python3 -m pytest -q
git add src/sonari/protocol.py src/sonari/paths.py src/sonari/daemon.py tests/
git commit -m "feat(daemon): caret tracking - mirror cursor over the open prompt with a virtual Submit; prompt-open flag for hotkeyd"
```

---

### Task 9: Caret tracking - hotkeyd side (listen-only arrow tap, permission flow)

**Files:**
- Modify: `hotkeyd/sonari-hotkeyd.swift`

No Python unit tests cover the Swift binary (repo convention - it's exercised by `sonari install` + the manual smoke). Keep the diff small and dumb, matching the file's existing comment style.

- [ ] **Step 1: Add the `--check-input-monitoring` mode** (for doctor; insert right after the imports/constants, before any setup):

```swift
// Phase 2.1: doctor probe. Exit 0 iff Input Monitoring is granted (the
// listen-only arrow tap needs it; everything else works without it).
if CommandLine.arguments.contains("--check-input-monitoring") {
    exit(CGPreflightListenEventAccess() ? 0 : 1)
}
```

- [ ] **Step 2: Add the listen-only arrow tap** (insert after the hotkey registration block, before `NSApplication.shared`):

```swift
// Phase 2.1: caret tracking. A LISTEN-ONLY CGEventTap observes arrow-key
// keyDown events (codes 125 down / 126 up) and forwards a caret_move message
// to speechd - but ONLY while ~/.sonari/prompt-open exists (the daemon
// creates/removes that flag around caret-trackable prompts). Listen-only
// means the event is NEVER consumed: the Claude Code TUI still receives
// every arrow press, so the mirror and the real highlight move together.
// Requires the Input Monitoring permission; without it the tap is skipped
// and Sonari runs exactly as before (hotkeys are Carbon, unaffected).
func promptOpenPath() -> String {
    return (sonariDir() as NSString).appendingPathComponent("prompt-open")
}

let arrowTapCallback: CGEventTapCallBack = { _, type, event, _ in
    if type == .keyDown {
        let code = event.getIntegerValueField(.keyboardEventKeycode)
        if (code == 125 || code == 126)
            && FileManager.default.fileExists(atPath: promptOpenPath()) {
            let dir = (code == 125) ? "down" : "up"
            sendMessage("{\"type\": \"caret_move\", \"dir\": \"\(dir)\"}")
        }
    }
    return Unmanaged.passUnretained(event)
}

if CGPreflightListenEventAccess() {
    let mask = CGEventMask(1 << CGEventType.keyDown.rawValue)
    if let tap = CGEvent.tapCreate(
        tap: .cgSessionEventTap,
        place: .headInsertEventTap,
        options: .listenOnly,
        eventsOfInterest: mask,
        callback: arrowTapCallback,
        userInfo: nil
    ) {
        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
        FileHandle.standardError.write(
            "hotkeyd: caret tap installed (listen-only)\n".data(using: .utf8)!)
    }
} else {
    // Ask for the permission ONCE per machine (the system shows its own
    // dialog and lists "sonari-hotkeyd" under Input Monitoring). A marker
    // file prevents re-prompting on every login.
    let marker = (sonariDir() as NSString)
        .appendingPathComponent(".input-monitoring-requested")
    if !FileManager.default.fileExists(atPath: marker) {
        FileManager.default.createFile(atPath: marker, contents: nil)
        _ = CGRequestListenEventAccess()
    }
    FileHandle.standardError.write(
        "hotkeyd: caret tracking disabled (Input Monitoring not granted)\n"
            .data(using: .utf8)!)
}
```

- [ ] **Step 3: Compile-check the Swift** (no install yet - just prove it builds):

Run: `swiftc hotkeyd/sonari-hotkeyd.swift -o /tmp/sonari-hotkeyd-build-check && /tmp/sonari-hotkeyd-build-check --check-input-monitoring; echo "exit=$?"`
Expected: compiles; exits 0 or 1 (either is fine - it proves the flag path works).

- [ ] **Step 4: Commit**

```bash
git add hotkeyd/sonari-hotkeyd.swift
git commit -m "feat(hotkeyd): listen-only arrow tap for caret tracking, gated by the prompt-open flag; one-time Input Monitoring request + doctor probe"
```

---

### Task 10: Doctor check for Input Monitoring

**Files:**
- Modify: `src/sonari/cli.py` (`doctor()`, after the "hotkeyd binary" check ~line 196)
- Test: `tests/test_cli_doctor.py` (append; read its existing monkeypatch style first and mirror it)

- [ ] **Step 1: Failing test** (append to `tests/test_cli_doctor.py`, following its existing fixture/monkeypatch conventions for faking binaries):

```python
def test_doctor_reports_input_monitoring_state(monkeypatch, tmp_path):
    from sonari import cli

    hk = tmp_path / "sonari-hotkeyd"
    hk.write_text("#!/bin/sh\nexit 0\n")
    hk.chmod(0o755)
    monkeypatch.setattr(cli.paths, "HOTKEYD_BIN_PATH", hk)
    rows = {name: (ok, detail) for name, ok, detail in cli.doctor()}
    ok, detail = rows["input monitoring (caret tracking)"]
    assert ok is True


def test_doctor_input_monitoring_not_granted(monkeypatch, tmp_path):
    from sonari import cli

    hk = tmp_path / "sonari-hotkeyd"
    hk.write_text("#!/bin/sh\nexit 1\n")
    hk.chmod(0o755)
    monkeypatch.setattr(cli.paths, "HOTKEYD_BIN_PATH", hk)
    rows = {name: (ok, detail) for name, ok, detail in cli.doctor()}
    ok, detail = rows["input monitoring (caret tracking)"]
    assert ok is False
    assert "Input Monitoring" in detail
```

- [ ] **Step 2: Run to verify failure.** `python3 -m pytest tests/test_cli_doctor.py -q -k input_monitoring`

- [ ] **Step 3: Implement** - in `doctor()`, insert after the "hotkeyd binary" block:

```python
    # Input Monitoring (Phase 2.1 caret tracking) - probe via the binary.
    if hk_exists:
        try:
            granted = subprocess.run(
                [hk_bin, "--check-input-monitoring"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                timeout=5,
            ).returncode == 0
            results.append((
                "input monitoring (caret tracking)", granted,
                "granted" if granted else
                "not granted - arrow-key narration off; grant in System "
                "Settings > Privacy & Security > Input Monitoring"))
        except Exception as exc:  # noqa: BLE001 - doctor must never raise
            results.append(("input monitoring (caret tracking)", False,
                            f"probe failed: {exc}"))
```

(Confirm `subprocess` is already imported in `cli.py`; add the import if not.)

- [ ] **Step 4: Full suite → green, commit**

```bash
python3 -m pytest -q
git add src/sonari/cli.py tests/test_cli_doctor.py
git commit -m "feat(doctor): report Input Monitoring state for caret tracking"
```

---

### Task 11: Docs + final suite

**Files:**
- Modify: `README.md` (hotkey table + a Phase 2.1 behavior note)
- Test: full suite under both interpreters

- [ ] **Step 1: Update README** - in the hotkeys/commands section: `repeat` = "re-speaks the entire last message"; `catch_up` = "replays everything you haven't heard (after stop, or from a session you left), then marks it heard"; `reread_options` = "re-reads the current prompt's options (numbers, descriptions, multi-select announce)"; add one new bullet: "Arrow keys inside a multi-select speak the highlighted option, including the Submit row (needs the one-time Input Monitoring permission - `sonari doctor` shows its state)." Match the README's existing terse table tone; no marketing prose.

- [ ] **Step 2: Run the dual-interpreter gate** (repo convention - system 3.9 AND a newer python if present):

```bash
/usr/bin/python3 -m pytest -q
python3 -m pytest -q
```

Expected: all green on both.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs(readme): Phase 2.1 - repeat/catch_up/reread semantics + caret tracking row"
```

---

### Task 12: Deploy + HUMAN LISTEN-TEST (STOP - needs Nima)

The spec's DoD items marked ⚠ are audible/behavioral - **not headlessly verifiable**. Do not claim them done from unit tests (escalate-the-unverifiable).

- [ ] **Step 1: Deploy to the live daemon** (the daemon runs from `~/.sonari/app`, not the repo):

```bash
cd ~/projects/private/claude-tts && ./bin/sonari install && sonari doctor
```

Expected: doctor green except possibly `input monitoring` → if not granted, grant it in System Settings > Privacy & Security > Input Monitoring (the hotkeyd prompt fires once), then `launchctl kickstart -k gui/$(id -u)/com.sonari.hotkeyd` and re-run doctor.

- [ ] **Step 2: Walk the listen-test with Nima** (live Claude Code session):
  1. Ask Claude something; mid-speech press **Ctrl+Cmd+R** → the **whole** last message restarts, not the last sentence.
  2. Press **Ctrl+Cmd+S** mid-sentence, then **Ctrl+Cmd+L** → playback resumes **from the start of the interrupted sentence**, oldest→newest; a second **L** says "You're all caught up."
  3. Trigger an AskUserQuestion (single + multi-select): the multi-select announces itself; options read with descriptions; after other speech, **Ctrl+Cmd+O** re-reads the options (not the queue tail); with no prompt open, **O** says "No options right now."
  4. In a multi-select, arrow ↓ through the options → each focused option is spoken; past the last option → "Submit."; Enter submits - fully eyes-free.
  5. Two sessions: prompt in A; while A speaks, switch and prompt in B → A keeps talking to the end (only Ctrl+Cmd+S stops it); B's answer is silent; in B, **Ctrl+Cmd+L** replays B's response ("Catching up on another session." if pressed from A).
- [ ] **Step 3: Record the outcome** in `docs/superpowers/phase21-listen-test.md` (pass/fail per item + anything that felt wrong by ear), commit, and use **superpowers:finishing-a-development-branch** to merge `phase-2.1-eyes-free-prompts` → `main` and push (Nima's call on push timing).

---

## Self-review notes

- **Spec coverage:** substrate → T2/T3; reread_options (slot, descriptions, multi-select announce, empty state) → T7; caret + virtual Submit → T8/T9; repeat whole-message → T5; catch_up (verbatim, marker, sentence-start resume, new-prompt reset, cross-session) → T6 + FLUSH in T3; voice continuity + silent capture + no-autostart → T4; bounded history → T2/T3 (`history_cap`); no-LLM-on-hotkey-path → trivially true (pure replay); human listen-test → T12. Spec's "refocus cue" deliberately dropped (justified in Deviations).
- **Type consistency checked:** `note_spoken(item, completed)`, `history.record/end_message/last_message/unheard/reset/other_session_with_unheard`, `_options: dict[str,str]`, `_caret{"session","labels","submit","pos"}`, `queue.clear()/flush_session()` returning lists - names match across all tasks.
- **Threading:** `handle_message` runs under `self._lock` (existing `_handle_conn`); `note_spoken` takes the lock itself - no nested locking (speak loop never calls `handle_message`).
- **Risk to verify live (T12):** the listen-only tap's one-time permission prompt from a LaunchAgent context, and TUI/mirror desync under page-scroll (mitigated by per-prompt snap-to-top; spec accepts as known risk).
