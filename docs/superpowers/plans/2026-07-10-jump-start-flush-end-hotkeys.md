# Jump-to-start / flush-to-end hotkeys Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two paired global hotkeys — Ctrl+Alt+Up (jump to start of the turn and replay) and Ctrl+Alt+Down (flush the engaged session's queue to the end, non-destructively) — and flip the Windows default hotkey modifier from Ctrl+Shift+Alt to Ctrl+Alt.

**Architecture:** Up reuses the daemon's existing `_nav(to="first")` seek-and-play path, so it needs only action→message→key wiring (no daemon logic). Down is a new `FLUSH_SESSION` protocol message with a new `handle_message` branch modeled on the existing `JUMP_DECISION`, advancing the engaged session's channel cursor to `len(items)` while popping skipped items' `_pending_heard` markers so they stay recoverable via catch-up. The modifier flip is a one-line change to the Windows backend's `DEFAULT_MODS`.

**Tech Stack:** Python 3.9+, stdlib only. pytest for tests. Windows `RegisterHotKey` backend (mockable — tests force `sys.platform`).

## Global Constraints

- Python 3.9+ compatible (no 3.10+ syntax). `from __future__ import annotations` is already used where needed.
- Windows-only runtime; tests force the platform via `monkeypatch.setattr(platform.sys, "platform", "win32")` and reset `platform._CACHE`.
- No new dependencies; stdlib only.
- No em-dashes in code comments or docs copy (project style: use en-dashes, commas, or rephrase).
- Both new actions are **hotkey-only** (no CLI command), mirroring `nav_prev` / `nav_next`.
- Both actions target `_engaged_session()` (the session the user HEARS), not the foreground.
- Flush is **non-destructive**: skipped items keep unheard history entries so catch-up / repeat recover them. Never `wipe()`.
- Run the whole suite from the venv: `.venv\Scripts\python -m pytest -q`.

---

### Task 1: Add the `FLUSH_SESSION` protocol message

**Files:**
- Modify: `src/sonara/protocol.py:9-39` (the `MsgType` class)
- Test: `tests/test_protocol.py:52-127` (two snapshot dicts)

**Interfaces:**
- Produces: `MsgType.FLUSH_SESSION == "flush_session"` — consumed by Task 2 (daemon handler) and Task 3 (keymap action message).

- [ ] **Step 1: Update the two snapshot tests to expect `FLUSH_SESSION`**

In `tests/test_protocol.py`, add one entry to BOTH expected dicts. In `test_msgtype_has_every_constant_with_exact_values` (after the `"NAV": "nav",` line, ~line 66) add:

```python
        "FLUSH_SESSION": "flush_session",
```

And in `test_msgtype_defines_no_extra_string_constants` (after the `"NAV": "nav",` line, ~line 108) add the same:

```python
        "FLUSH_SESSION": "flush_session",
```

- [ ] **Step 2: Run the snapshot tests to verify they fail**

Run: `.venv\Scripts\python -m pytest tests/test_protocol.py -q`
Expected: FAIL — `test_msgtype_has_every_constant_with_exact_values` (`MsgType missing FLUSH_SESSION`) and `test_msgtype_defines_no_extra_string_constants` (dict inequality).

- [ ] **Step 3: Add the constant to `MsgType`**

In `src/sonara/protocol.py`, add the constant right after the `NAV` line (line 22), keeping the inline-comment style:

```python
    FLUSH_SESSION = "flush_session"   # hotkey: flush the engaged session's queue to the end
```

- [ ] **Step 4: Run the protocol tests to verify they pass**

Run: `.venv\Scripts\python -m pytest tests/test_protocol.py -q`
Expected: PASS (all, including `test_msgtype_values_are_unique`).

- [ ] **Step 5: Commit**

```bash
git add src/sonara/protocol.py tests/test_protocol.py
git commit -m "feat(protocol): add FLUSH_SESSION message type"
```

---

### Task 2: Add the `FLUSH_SESSION` daemon handler (flush to end)

**Files:**
- Modify: `src/sonara/daemon.py` — add a branch in `handle_message()` immediately after the `JUMP_DECISION` branch (which ends at line 597, before the `CATCH_UP` branch at line 599).
- Test: `tests/test_daemon_flush_session.py` (new file)

**Interfaces:**
- Consumes: `MsgType.FLUSH_SESSION` (Task 1); existing `self._engaged_session()`, `self.router.channel(session)`, `self._pending_heard` (dict id→history entry), `self._current_item`, `self.speaker.cancel()`, `self._earcon(kind)`, `self._wake.set()`, `SessionChannel.cursor` / `.items` / `.has_decision`.
- Produces: handling of `{"type": "flush_session"}` — after handling, the engaged channel's `cursor == len(items)`, the current utterance is cancelled iff it belongs to the engaged session, skipped items' `_pending_heard` markers are popped (stay unheard in history), and a `nav` earcon fires when anything was flushed (`nav_edge` when nothing was).

- [ ] **Step 1: Write the failing tests**

Create `tests/test_daemon_flush_session.py`:

```python
"""Ctrl+Alt+Down = flush to end: skip ALL pending items for the engaged session
and go idle, non-destructively (skipped items stay unheard so catch-up recovers
them). Models JUMP_DECISION but advances the cursor to the very end."""
from sonara.protocol import MsgType, PROTOCOL_VERSION
from sonara.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _flush(daemon, session="fg"):
    daemon.handle_message({"type": MsgType.FLUSH_SESSION, "session": session})


def _prose(session, text, idx):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": session,
            "delta": text, "index": idx, "final": True}


def test_flush_advances_cursor_to_end_and_cancels_current():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=1, session="fg", kind="prose", text="a", is_decision=False))
    ch.append(SpeechItem(id=2, session="fg", kind="prose", text="b", is_decision=False))
    daemon._current_item = SpeechItem(id=3, session="fg", kind="prose",
                                      text="cur", is_decision=False)
    _flush(daemon)
    assert ch.cursor == len(ch.items)        # nothing pending
    assert ch.pending() == 0
    assert speaker.cancels == 1              # current utterance cut
    assert speaker.earcons[-1] == "nav"


def test_flush_does_not_cancel_when_current_is_another_session():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=1, session="fg", kind="prose", text="a", is_decision=False))
    daemon._current_item = SpeechItem(id=9, session="other", kind="prose",
                                      text="x", is_decision=False)
    _flush(daemon)
    assert ch.cursor == len(ch.items)        # fg still drained
    assert speaker.cancels == 0              # other session's audio untouched


def test_flush_clears_pending_decision_flag():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=1, session="fg", kind="permission", text="ok?",
                         is_decision=True))
    assert ch.has_decision is True
    _flush(daemon)
    assert ch.has_decision is False


def test_flush_with_nothing_pending_is_a_safe_edge_no_op():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    _flush(daemon)
    assert daemon.router.channel("fg").cursor == 0
    assert speaker.earcons[-1] == "nav_edge"


def test_flush_with_no_engaged_session_edges():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground=None)
    daemon.handle_message({"type": MsgType.FLUSH_SESSION, "session": "x"})
    assert speaker.earcons[-1] == "nav_edge"


def test_flushed_items_stay_recoverable_via_catch_up():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message(_prose("fg", "One. ", 0))
    daemon.handle_message(_prose("fg", "Two. ", 1))
    _flush(daemon)
    assert daemon.router.channel("fg").pending() == 0        # flushed
    daemon.handle_message({"type": MsgType.CATCH_UP, "session": "fg"})
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert "One." in texts and "Two." in texts               # catch-up brought them back
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `.venv\Scripts\python -m pytest tests/test_daemon_flush_session.py -q`
Expected: FAIL — the `flush_session` message is unhandled, so cursors don't advance / earcons aren't emitted (assertions fail; no branch exists yet).

- [ ] **Step 3: Add the handler branch**

In `src/sonara/daemon.py`, immediately AFTER the `JUMP_DECISION` branch's `return None` (line 597) and BEFORE `if t == MsgType.CATCH_UP:` (line 599), insert:

```python
        if t == MsgType.FLUSH_SESSION:
            # Flush to end: skip ALL pending items for the engaged session and go
            # idle. Non-destructive: skipped items keep their history entries
            # UNHEARD (we pop their _pending_heard markers so note_spoken never
            # flips them True), so CATCH_UP / REPEAT can bring them back. Mirrors
            # JUMP_DECISION but advances the cursor to the very end, not the next
            # decision. Nothing is wiped; this is a cursor move.
            fg = self._engaged_session()
            if fg is None:
                self._earcon("nav_edge")
                return None
            ch = self.router.channel(fg)
            skipped = 0
            while ch.cursor < len(ch.items):
                self._pending_heard.pop(ch.items[ch.cursor].id, None)
                ch.cursor += 1
                skipped += 1
            ch.has_decision = False        # any pending decision was skipped
            cur = self._current_item
            cutting = cur is not None and cur.session == fg
            if cutting:
                self.speaker.cancel()      # cut the in-progress utterance for fg
            self._earcon("nav" if (skipped or cutting) else "nav_edge")
            self._wake.set()
            return None
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `.venv\Scripts\python -m pytest tests/test_daemon_flush_session.py -q`
Expected: PASS (all 6).

- [ ] **Step 5: Run the daemon + protocol suites for regressions**

Run: `.venv\Scripts\python -m pytest tests/test_daemon_nav.py tests/test_daemon_channels.py tests/test_hotkeyd_contract.py -q`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/sonara/daemon.py tests/test_daemon_flush_session.py
git commit -m "feat(daemon): FLUSH_SESSION flushes the engaged session's queue to the end"
```

---

### Task 3: Wire the `nav_start` and `flush` hotkey actions

**Files:**
- Modify: `src/sonara/keymap.py:26-35` (`ACTION_MESSAGES`) and `src/sonara/keymap.py:42-45` (`_DEFAULT_KEYS`)
- Test: `tests/test_keymap.py` (add new tests; update `test_default_keymap_binds_only_nav_pause_mute` at line 64)

**Interfaces:**
- Consumes: `MsgType.FLUSH_SESSION == "flush_session"` (Task 1) — the `flush` action's message type must be a known `MsgType`, enforced by `tests/test_hotkeyd_contract.py::test_all_action_messages_are_known_msgtypes`.
- Produces: `ACTION_MESSAGES["nav_start"] == {"type": "nav", "to": "first"}`, `ACTION_MESSAGES["flush"] == {"type": "flush_session"}`, default keys `nav_start → "up"`, `flush → "down"`.

- [ ] **Step 1: Write the failing tests**

Add to `tests/test_keymap.py` (end of file):

```python
def test_nav_start_action_message_is_nav_first():
    assert keymap.ACTION_MESSAGES["nav_start"] == {"type": "nav", "to": "first"}


def test_flush_action_message():
    assert keymap.ACTION_MESSAGES["flush"] == {"type": "flush_session"}


def test_nav_start_and_flush_default_to_up_and_down():
    km = keymap.default_keymap()
    assert km["nav_start"]["key"] == "up"
    assert km["flush"]["key"] == "down"


def test_arrow_cluster_default_keys_are_distinct():
    km = keymap.default_keymap()
    arrows = {km[a]["key"] for a in ("nav_prev", "nav_next", "nav_start", "flush")}
    assert arrows == {"left", "right", "up", "down"}
```

Then UPDATE the existing `test_default_keymap_binds_only_nav_pause_mute` (line 64) to include the two new default bindings. Replace its `set(km.keys()) == {...}` assertion with:

```python
    assert set(km.keys()) == {"nav_prev", "nav_next", "nav_start", "flush",
                              "mute", "next_session"}
```

- [ ] **Step 2: Run the keymap tests to verify the new/updated ones fail**

Run: `.venv\Scripts\python -m pytest tests/test_keymap.py -q`
Expected: FAIL — `KeyError`/missing-key on `nav_start`/`flush`, and the updated `test_default_keymap_binds_only_nav_pause_mute` mismatch.

- [ ] **Step 3: Add the two actions and their default keys**

In `src/sonara/keymap.py`, extend `ACTION_MESSAGES` (after the `nav_prev` line, line 29):

```python
    "nav_start": {"type": "nav", "to": "first"},   # jump to start of turn + replay
    "flush": {"type": "flush_session"},            # flush the engaged session to the end
```

And extend `_DEFAULT_KEYS` (after the `nav_prev`/`nav_next` line, line 43):

```python
    "nav_start": "up", "flush": "down",
```

- [ ] **Step 4: Run the keymap + contract tests to verify they pass**

Run: `.venv\Scripts\python -m pytest tests/test_keymap.py tests/test_hotkeyd_contract.py -q`
Expected: PASS (including `test_all_action_messages_are_known_msgtypes` and `test_no_two_default_actions_share_a_key`).

- [ ] **Step 5: Commit**

```bash
git add src/sonara/keymap.py tests/test_keymap.py
git commit -m "feat(keymap): bind nav_start (Up) and flush (Down) actions"
```

---

### Task 4: Flip the Windows default modifier to Ctrl+Alt

**Files:**
- Modify: `src/sonara/platform/windows/keytables.py:27-28` (`DEFAULT_MODS`)
- Test: update `tests/test_win_keytables.py:16-17`, `tests/test_win_backend.py:18`, `tests/test_win_hotkeys.py:39`, `tests/test_keymap.py:47-49`

**Interfaces:**
- Consumes: nothing new.
- Produces: `keytables.DEFAULT_MODS == ["ctrl", "alt"]`, so `WinHotkeyBackend.default_mods()` and every default binding (Left/Right/Up/Down/M/P) resolve without Shift.

- [ ] **Step 1: Update the tests that pin the old Ctrl+Shift+Alt default**

In `tests/test_win_keytables.py`, replace the body + name of the test at lines 16-17:

```python
def test_default_mods_is_ctrl_alt():
    assert wk.DEFAULT_MODS == ["ctrl", "alt"]
```

In `tests/test_win_backend.py` line 18, change to:

```python
    assert hk.default_mods() == ["ctrl", "alt"]
```

In `tests/test_win_hotkeys.py` line 39, change to:

```python
    assert hk.default_mods() == ["ctrl", "alt"]
```

In `tests/test_keymap.py`, replace the test at lines 47-49 (name + assertion):

```python
def test_default_keymap_windows_uses_ctrl_alt(win):
    d = keymap.default_keymap()
    assert d["nav_next"]["mods"] == ["ctrl", "alt"]
    assert d["mute"]["key"] == "m"
```

(Leave `tests/test_win_hotkeys.py:117`'s `display_combo(... ) == "Ctrl+Shift+Alt+O"` unchanged — it passes explicit modifier bits, not the default chord.)

- [ ] **Step 2: Run the affected tests to verify they fail**

Run: `.venv\Scripts\python -m pytest tests/test_win_keytables.py tests/test_win_backend.py tests/test_win_hotkeys.py tests/test_keymap.py -q`
Expected: FAIL — the updated assertions expect `["ctrl", "alt"]` but the source still returns `["ctrl", "shift", "alt"]`.

- [ ] **Step 3: Change the default chord**

In `src/sonara/platform/windows/keytables.py`, replace lines 27-28:

```python
# Default chord: Ctrl+Alt clears AltGr / Win-reserved / terminal / layout collisions.
DEFAULT_MODS = ["ctrl", "alt"]
```

- [ ] **Step 4: Run the affected tests to verify they pass**

Run: `.venv\Scripts\python -m pytest tests/test_win_keytables.py tests/test_win_backend.py tests/test_win_hotkeys.py tests/test_keymap.py -q`
Expected: PASS.

- [ ] **Step 5: Run the full suite to catch any other modifier assumptions**

Run: `.venv\Scripts\python -m pytest -q`
Expected: PASS (whole suite green).

- [ ] **Step 6: Commit**

```bash
git add src/sonara/platform/windows/keytables.py tests/test_win_keytables.py tests/test_win_backend.py tests/test_win_hotkeys.py tests/test_keymap.py
git commit -m "feat(hotkeys): default modifier Ctrl+Alt (drop Shift) for all bindings"
```

---

### Task 5: Update the README

**Files:**
- Modify: `README.md` (global hotkeys section ~lines 104-121, per-session section ~lines 183-188)

**Interfaces:** none (docs only). No CLI/keymap.md change — `commands/keymap.md` does not hardcode the modifier or the action list.

- [ ] **Step 1: Update the modifier sentence and the hotkey table**

In `README.md`, change the modifier line (line 106) from:

```
Default modifier is **Ctrl+Shift+Alt** (rebindable via `~/.sonara/keymap.json`). The daemon
```

to:

```
Default modifier is **Ctrl+Alt** (rebindable via `~/.sonara/keymap.json`). The daemon
```

Then replace the hotkey table (lines 115-121) with the six-row version (note the two new Up/Down rows and the Ctrl+Alt prefix everywhere):

```
| Hotkey | Effect |
|---|---|
| Ctrl+Alt+Left | Previous item — step back through the current turn |
| Ctrl+Alt+Right | Next item — step forward through the current turn |
| Ctrl+Alt+Up | Jump to the start of the current turn and replay from the top |
| Ctrl+Alt+Down | Flush — skip the rest of this session's queue and go quiet (recoverable via catch-up) |
| Ctrl+Alt+M | Cycle mute: Unmuted → Muted (speech) → Super muted (speech + beeps) |
| Ctrl+Alt+P | Cycle to the next session in a fixed round-robin (resumes an unread session, replays a read one). Says "Session changed: &lt;folder&gt;." |
```

- [ ] **Step 2: Update the remaining Ctrl+Shift+Alt mention in the per-session section**

In `README.md` (line 185), change:

```
**Ctrl+Shift+Alt+P**. Sonara advances to the next session in a
```

to:

```
**Ctrl+Alt+P**. Sonara advances to the next session in a
```

Also update the "Only four actions are bound by default" note (lines 109-113) to reflect that Up/Down are now bound too — change "Only four actions are bound by default" to "Only these actions are bound by default" and keep the pause/faster/slower unbound note as-is.

- [ ] **Step 3: Verify no stale `Ctrl+Shift+Alt` remains in the README**

Run: `git grep -n "Ctrl+Shift+Alt" README.md`
Expected: no output (all replaced).

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: Ctrl+Alt modifier + Up/Down (start/flush) hotkeys in README"
```

---

## Self-Review

**Spec coverage:**
- Ctrl+Alt+Up = jump to start (reuse `_nav` first) → Task 3 (action wiring; daemon already handles `nav to=first`, covered by existing `test_daemon_nav.py::test_first_and_last_jump`). ✓
- Ctrl+Alt+Down = flush to end, non-destructive, recoverable → Task 2 (handler + recovery test) + Task 3 (binding). ✓
- Modifier Ctrl+Shift+Alt → Ctrl+Alt for all defaults → Task 4. ✓
- `FLUSH_SESSION` MsgType → Task 1. ✓
- Engaged-session targeting → Task 2 (uses `_engaged_session()`; `test_flush_with_no_engaged_session_edges`). ✓
- Skipped items stay unheard/recoverable → Task 2 (`test_flushed_items_stay_recoverable_via_catch_up`). ✓
- Hotkey-only, no CLI → no CLI task; noted in Global Constraints. ✓
- Docs (README) → Task 5. `commands/keymap.md` needs no change (generic). ✓
- Tests: protocol snapshot (T1), keymap resolve + defaults + modifier (T3/T4), daemon handler (T2), nav-first wiring exists (T3 note). ✓

**Placeholder scan:** No TBD/TODO; every code step shows the exact code and the exact insertion anchor. ✓

**Type consistency:** `FLUSH_SESSION`/`"flush_session"`, `nav_start`/`{"type":"nav","to":"first"}`, `flush`/`{"type":"flush_session"}`, and the handler symbols (`_engaged_session`, `router.channel`, `_pending_heard`, `_current_item`, `speaker.cancel`, `_earcon`, `_wake`, `SessionChannel.cursor/items/has_decision`) match the daemon signatures verified in the source. Default keys `up`/`down` exist in `keytables.KEY_CODES`. ✓
