# Session-change Hotkey Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the pin hotkey with a `next_session` hotkey that cycles the active reader through sessions in a fixed round-robin ring (resume unread, replay read), eliminating the `repin_reset` cursor-replay bug.

**Architecture:** Add `Router.next_session()` (pure round-robin selection that sets `active` and arms the existing session-change announcement), wire a `NEXT_SESSION` protocol message + daemon handler + keymap binding to it, then remove the pin feature wholesale. The switch "sticks" via the router's existing current-reader-keeps-the-floor rule - no lock, no cursor reset except the deliberate replay case.

**Tech Stack:** Python 3.9+ stdlib. pytest. Existing per-session channel/router daemon.

## Global Constraints

- Python **>= 3.9**; string type annotations (e.g. `"str | None"`).
- Default keybinding for `next_session` is **Ctrl+Alt+P** (the key pin freed) - reuse the existing `"p"` default-key slot.
- Announcement copy: normal switch = `"Session changed: {folder}."`; replay (landing on a read session) = `"Session changed: {folder}, reading again."`; no-channels cue = `"No session."`.
- Read = `channel.caught_up()` (cursor at end). Unread = `channel.pending() > 0`. No new "read" flag.
- The session-change announcement reuses the `session_change` SpeechItem kind (fires the chime) and is `mute_exempt=True`.
- Known pre-existing Windows-environmental test failures (test_bin_shims, test_bin_sonari, test_daemon_main::test_ensure_running…, test_kokoro_provision, test_paths, test_transport, test_win_autostart, test_win_tts) are NOT in scope - leave them.
- Spec: `docs/superpowers/specs/2026-06-20-session-change-hotkey-design.md`.

---

### Task 1: `Router.next_session()` + replay-aware announcement

**Files:**
- Modify: `src/sonari/router.py`
- Test: `tests/test_router.py`

**Interfaces:**
- Consumes: `SessionChannel` (`pending()`, `caught_up()`, `reset()`, `next()`); `self.channels` (insertion-ordered dict), `self.active`, `self._pending_announce`, `CONTROL`.
- Produces: `Router.next_session() -> "tuple[str | None, bool]"` returning `(target, replay)`; `(None, False)` when no other session. `announce_text` is now called as `announce_text(folder, replay)`.

- [ ] **Step 1: Write the failing tests**

```python
# add to tests/test_router.py
def test_next_session_advances_one_slot_in_fixed_order():
    r, s = _router()
    for name in ("A", "B", "C"):
        ch = r.channel(name); ch.append(_item(name, name.lower())); ch.turn_done = True
    r.active = "A"
    assert r.next_session()[0] == "B"              # A -> B (next in insertion order)
    assert r.next_session()[0] == "C"              # B -> C
    assert r.next_session()[0] == "A"              # C -> A (wrap)


def test_next_session_resumes_an_unread_target_no_replay():
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.append(_item("B", "b2")); b.turn_done = True
    b.next()                                       # B partially read (still unread)
    r.active = "A"
    target, replay = r.next_session()
    assert (target, replay) == ("B", False)        # unread -> resume, not replay
    assert r.channels["B"].cursor == 1             # cursor NOT reset
    assert r.active == "B"


def test_next_session_replays_a_read_target():
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True
    b = r.channel("B"); b.append(_item("B", "b1")); b.turn_done = True
    b.next()                                       # B fully read (caught up)
    r.active = "A"
    assert r.channels["B"].caught_up() is True
    target, replay = r.next_session()
    assert (target, replay) == ("B", True)         # read -> replay
    assert r.channels["B"].cursor == 0             # cursor reset for replay


def test_next_session_single_session_lands_on_itself():
    r, s = _router()
    a = r.channel("A"); a.append(_item("A", "a1")); a.turn_done = True; a.next()  # read
    r.active = "A"
    assert r.next_session() == ("A", True)         # wraps to itself; read -> replay


def test_next_session_none_when_no_channels():
    r, s = _router()
    assert r.next_session() == (None, False)       # nothing registered
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/test_router.py -q -k next_session`
Expected: FAIL - `AttributeError: 'Router' object has no attribute 'next_session'`

- [ ] **Step 3: Implement `next_session` + replay announcement**

In `src/sonari/router.py`, add to `__init__` (next to `_pending_announce`):

```python
        self._pending_announce_replay = False
```

Add the method (place after `repin_reset`):

```python
    def next_session(self) -> "tuple[str | None, bool]":
        """Manual session-change: a pure round-robin. Advance the active reader to
        the next session after the current one in a FIXED order (channel insertion
        order, excluding CONTROL), wrapping; with one session it lands on itself.
        A read (caught-up) target is reset to 0 and replayed (replay=True); an
        unread target resumes from its cursor (replay=False). Returns (None, False)
        only when there are no channels. Arms the session-change announcement."""
        keys = [s for s in self.channels if s != CONTROL]
        if not keys:
            return (None, False)
        if self.active in keys:
            i = keys.index(self.active)
            target = keys[(i + 1) % len(keys)]     # next in the fixed ring (wraps)
        else:
            target = keys[0]
        replay = self.channels[target].caught_up()
        if replay:
            self.channels[target].reset()
        self._arm_switch(target, replay)
        return (target, replay)

    def _arm_switch(self, target: str, replay: bool) -> None:
        self.active = target
        self._last_active = target                 # auto won't re-announce after
        self._pending_announce = target
        self._pending_announce_replay = replay
```

Update the `_pending_announce` block in `next_item` to pass the replay flag and clear it:

```python
        if self._pending_announce is not None:
            folder = self.sessions.folder(self._pending_announce) or "another session"
            text = self._announce_text(folder, self._pending_announce_replay)
            self._pending_announce = None
            self._pending_announce_replay = False
            return SpeechItem(id=0, session=self.active or "", kind="session_change",
                              text=text, is_decision=False, mute_exempt=True)
```

Update the AUTO-handoff announce (later in `next_item`) to pass `False`:

```python
                self._pending_announce = target
                self._last_active = target
                return self.next_item()
```
(unchanged - it sets `_pending_announce`; `_pending_announce_replay` stays False by default, so the block above formats the non-replay text.)

Update the `_router()` helper at the top of `tests/test_router.py` so `announce_text` takes the replay flag:

```python
    r = Router(s, minqueue=lambda: mq,
               announce_text=lambda f, replay=False: (
                   "Session changed: {0}, reading again.".format(f) if replay
                   else "Session changed: {0}.".format(f)))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_router.py -q`
Expected: PASS (existing router tests + 5 new ones)

- [ ] **Step 5: Commit**

```bash
git add src/sonari/router.py tests/test_router.py
git commit -m "feat(core): Router.next_session - round-robin select + replay announce (#59)"
```

---

### Task 2: Wire `NEXT_SESSION` end-to-end (protocol + daemon + keymap + debounce)

**Files:**
- Modify: `src/sonari/protocol.py`, `src/sonari/daemon.py`, `src/sonari/keymap.py`
- Test: `tests/test_daemon_channels.py` (or a new `tests/test_daemon_session_change.py`), `tests/test_protocol.py`, `tests/test_keymap.py`

**Interfaces:**
- Consumes: `Router.next_session()` (Task 1); the daemon's `announce_text` lambda; `_speak_cue`; `_DEBOUNCED_HOTKEYS`.
- Produces: `MsgType.NEXT_SESSION = "next_session"`; daemon handles it; keymap action `"next_session"` bound to `"p"`.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_daemon_session_change.py
from tests.daemon_helpers import make_daemon
from sonari.protocol import MsgType, PROTOCOL_VERSION


def _prose(s, d, i, f):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s, "delta": d, "index": i, "final": f}


def _spoken(daemon, speaker, n=12):
    for _ in range(n):
        daemon._speak_loop_once()
    return speaker.spoken


def test_next_session_switches_to_other_unread_and_announces():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "A one. ", 0, True))
    daemon.handle_message(_prose("B", "B one. ", 0, True))
    daemon._speak_loop_once()                    # start reading (A or B)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.NEXT_SESSION})
    out = _spoken(daemon, speaker)
    assert any(t.startswith("Session changed:") for t in out)
    assert "A one." in out and "B one." in out   # both heard, nothing lost


def test_next_session_with_no_channels_speaks_cue():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")   # no prose -> no channels yet
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.NEXT_SESSION})
    out = _spoken(daemon, speaker)
    assert "No session." in out


def test_next_session_revisit_read_session_says_reading_again():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    # give folders so the announcement names them
    for s, cwd in (("A", "/u/alpha"), ("B", "/u/beta")):
        daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START,
                               "session": s, "cwd": cwd, "plugin_version": ""})
    daemon.handle_message(_prose("A", "A one. ", 0, True))
    daemon.handle_message(_prose("B", "B one. ", 0, True))
    _spoken(daemon, speaker, 12)                  # drain both -> both read
    speaker.spoken.clear()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.NEXT_SESSION})
    out = _spoken(daemon, speaker, 6)
    assert any("reading again" in t for t in out)


def test_next_session_is_debounced():
    from sonari.protocol import MsgType
    daemon = make_daemon()[0]
    assert daemon._debounce_suppress(MsgType.NEXT_SESSION, 1.0) is False
    assert daemon._debounce_suppress(MsgType.NEXT_SESSION, 1.10) is True   # rapid repeat dropped
```

```python
# add to tests/test_keymap.py
def test_next_session_action_message():
    from sonari.keymap import ACTION_MESSAGES
    assert ACTION_MESSAGES["next_session"] == {"type": "next_session"}


def test_next_session_default_binding_is_p():
    from sonari.keymap import default_keymap
    km = default_keymap()
    assert km["next_session"]["key"] == "p"
```

```python
# add to tests/test_protocol.py exhaustive manifest (both the has-every and no-extra tests)
        "NEXT_SESSION": "next_session",
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/test_daemon_session_change.py tests/test_keymap.py::test_next_session_action_message -q`
Expected: FAIL - no `NEXT_SESSION` / no `next_session` action.

- [ ] **Step 3: Implement the wiring**

`src/sonari/protocol.py` - add next to `PIN_TOGGLE` (keep PIN_TOGGLE for now; Task 3 removes it):

```python
    NEXT_SESSION = "next_session"   # hotkey: cycle the active reader to another session
```

`src/sonari/keymap.py` - add to `ACTION_MESSAGES`:

```python
    "next_session": {"type": "next_session"},   # cycle the active reader (replaces pin)
```

and rebind the default key (change the existing `"pin_toggle": "p"` to `next_session`):

```python
    "pause": "s", "mute": "m", "next_session": "p",
```

`src/sonari/daemon.py` - update `_DEBOUNCED_HOTKEYS` (swap PIN_TOGGLE for NEXT_SESSION):

```python
_DEBOUNCED_HOTKEYS = (
    MsgType.PAUSE, MsgType.MUTE, MsgType.NEXT_SESSION, MsgType.CYCLE_VERBOSITY,
)
```

`src/sonari/daemon.py` - update the `announce_text` lambda in `__init__` to accept the replay flag:

```python
            announce_text=lambda folder, replay=False: (
                "Session changed: {0}, reading again.".format(folder) if replay
                else "Session changed: {0}.".format(folder)),
```

`src/sonari/daemon.py` - add the `NEXT_SESSION` handler (place right after the existing `PIN_TOGGLE` handler):

```python
        if t == MsgType.NEXT_SESSION:
            # Manual session-change: switch the active reader to another session and
            # confirm immediately (cancel the current item, like pause/mute). The
            # router arms the "Session changed" announcement; on no other session we
            # speak a soft cue.
            target, _replay = self.router.next_session()
            self.speaker.cancel()
            if target is None:
                self._speak_cue(None, "No session.", exempt_mute=True)
            self._wake.set()
            return None
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_daemon_session_change.py tests/test_keymap.py tests/test_protocol.py -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sonari/protocol.py src/sonari/keymap.py src/sonari/daemon.py tests/test_daemon_session_change.py tests/test_keymap.py tests/test_protocol.py
git commit -m "feat(core): NEXT_SESSION hotkey wired end-to-end (protocol/daemon/keymap) (#59)"
```

---

### Task 3: Remove the pin feature

**Files:**
- Modify: `src/sonari/sessions.py`, `src/sonari/router.py`, `src/sonari/protocol.py`, `src/sonari/daemon.py`, `src/sonari/keymap.py`
- Test: delete `tests/test_daemon_pin.py`; update `tests/test_keymap.py`, `tests/test_protocol.py`, and any `tests/test_router.py` referencing pin.

**Interfaces:**
- Consumes: nothing new.
- Produces: no `pin_toggle`/`pinned`/`_pinned`/`PIN_TOGGLE`/`repin_reset` anywhere.

- [ ] **Step 1: Remove pin from `SessionManager`**

In `src/sonari/sessions.py`: delete `pin_toggle()`, `pinned()`, the `self._pinned` field, and simplify `foreground()` to `return self._foreground`. In `unregister()`, delete the `if self._pinned == session: self._pinned = None` lines.

- [ ] **Step 2: Remove pin from `Router`**

In `src/sonari/router.py`: delete `repin_reset()`. In `_pick`, delete the leading lines:

```python
        pinned = self.sessions.pinned()
        if pinned is not None:
            return pinned if pinned in self.channels else None
```

In `next_item`'s AUTO-handoff announce condition, drop the `self.sessions.pinned() is None` term so it reads:

```python
            if (self._last_active is not None and target != self._last_active):
```

- [ ] **Step 3: Remove pin from protocol / daemon / keymap**

`src/sonari/protocol.py`: delete the `PIN_TOGGLE = "pin_toggle"` line.
`src/sonari/keymap.py`: delete the `"pin_toggle": {"type": "pin_toggle"}` line from `ACTION_MESSAGES`.
`src/sonari/daemon.py`: delete the entire `if t == MsgType.PIN_TOGGLE:` handler block.

- [ ] **Step 4: Update/remove tests**

Delete `tests/test_daemon_pin.py`. In `tests/test_keymap.py`: change the two default-binding set assertions (lines ~66, ~107) from `"pin_toggle"` to `"next_session"`, and delete `test_pin_toggle_action_message` / `test_pin_toggle_default_binding_is_p` (replaced by Task 2's next_session tests). In `tests/test_protocol.py`: remove the `"PIN_TOGGLE": "pin_toggle"` entry from both manifest dicts. Grep for stragglers: `grep -rn "pin_toggle\|PIN_TOGGLE\|repin_reset\|\.pinned(" tests/ src/` must return nothing (except this plan/spec docs).

- [ ] **Step 5: Run the full suite**

Run: `python -m pytest -q`
Expected: green except the known pre-existing Windows-environmental failures. Fix any straggler that referenced pin.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(core): remove the pin feature (superseded by next_session) (#59)"
```

---

### Task 4: Docs + multi-session integration test

**Files:**
- Modify: `README.md` (and any `docs/` keymap reference that lists pin)
- Test: `tests/test_daemon_multisession.py`

**Interfaces:**
- Consumes: the full daemon.

- [ ] **Step 1: Write the integration test**

```python
# add to tests/test_daemon_multisession.py
def test_session_change_cycles_and_revisits():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    for s, cwd in (("A", "/u/alpha"), ("B", "/u/beta")):
        daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START,
                               "session": s, "cwd": cwd, "plugin_version": ""})
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "A"})
    daemon.handle_message(_prose("A", "Alpha message. "))
    daemon.router.channel("A").turn_done = True
    daemon.handle_message(_prose("B", "Beta message. "))
    daemon.router.channel("B").turn_done = True
    # press next-session: should switch to the other unread session, announced + chimed
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.NEXT_SESSION})
    for _ in range(12):
        daemon._speak_loop_once()
    assert "Alpha message." in speaker.spoken and "Beta message." in speaker.spoken
    assert "session_change" in speaker.earcons      # chime fired on the manual switch
```

- [ ] **Step 2: Run it (write → pass)**

Run: `python -m pytest tests/test_daemon_multisession.py -q`
Expected: PASS

- [ ] **Step 3: Update docs**

In `README.md`, replace the pin hotkey row/description with the session-change hotkey: `Ctrl+Alt+P` (or platform chord) = "Cycle the voice to the next session in a fixed round-robin (resumes an unread one, replays a read one). Says 'Session changed: <folder>.'". Remove any "pin"/"Pinned" wording. Grep `grep -rni "pin" README.md docs/` and fix living references (ignore the dated spec/plan files).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs+test(core): session-change hotkey docs + multisession integration (#59)"
```

---

## Self-review notes

- Spec §2 (behavior: pass1/2/3) → Task 1 (router selection) + Task 2 (daemon cue/announce). §3 (sticks via current-reader-keeps-floor) → Task 1 sets `active`, no lock added. §4 (components, removals) → Task 2 (add) + Task 3 (remove pin). §5 edge cases → Task 1 tests (none/wrap) + Task 2 (no-other cue) + Task 4 (integration). §6 testing → every task. §7 rollout (instrumentation already reverted) → nothing to remove.
- Type consistency: `next_session() -> (str|None, bool)` used identically in Tasks 1, 2, 4. `announce_text(folder, replay=False)` updated in both the daemon lambda (Task 2) and the test helper (Task 1). `MsgType.NEXT_SESSION = "next_session"` and the keymap action `"next_session"` match.
- Ordering: additive (1, 2) before removal (3) so the suite stays green at each task; docs/integration last (4).
