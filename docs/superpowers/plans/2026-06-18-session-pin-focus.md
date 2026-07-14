# Session Pin Focus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a single global hotkey that pins Sonari's voice to the session the user is currently working in (press again to unpin → auto).

**Architecture:** A "pin" overrides what "foreground" means. `SessionManager` gains `_pinned` and a `pin_toggle()`; `foreground()`/`is_foreground()` return the pin when set. Because the daemon's voice-ownership gate (`_may_speak`) already keys on `is_foreground`, no voice-ownership logic changes - the pin flows through the existing gate. A new `PIN_TOGGLE` protocol message (hotkey → daemon) triggers the toggle and a spoken confirmation; the hook starts sending `cwd` so the confirmation can name the folder.

**Tech Stack:** Python ≥3.9, stdlib only. Pytest. Existing seams: `sonari.sessions`, `sonari.daemon`, `sonari.protocol`, `sonari.keymap`, `sonari.hooks_entry`.

## Global Constraints

- Python ≥3.9; stdlib only (no new deps). `from __future__ import annotations` is in use - annotations are lazy strings.
- Spec: `docs/superpowers/specs/2026-06-18-session-pin-focus-design.md`. Issue: nimkimi/sonari#31.
- Default state is **auto** (nothing pinned → foreground = last prompt). Pinned session keeps the voice when others submit; pinned session ends → auto.
- Folder name = portable basename of `cwd` (handle both `/` and `\` separators regardless of host OS).
- Earcon kinds are fixed at 6 (a test asserts exactly 6) - do **not** add a new earcon kind; reuse `"error"`.
- Default keybind for the new action is `f` (NOT `p` - `p` is taken by `pause`). The shared default needs Nima's sign-off per CONTRIBUTING; the user's personal binding is separate.
- Run on Windows before opening; "green" = zero new failures vs `main`'s baseline (the known macOS-codebase-on-Windows env failures). `skipif`-guard Win32-only tests; `opt` not `alt` on macOS.
- Squash-merge, one concern, branch `core/session-pin`. Closes #31.

---

### Task 1: SessionManager - cwd capture + pin state + pin-aware foreground

**Files:**
- Modify: `src/sonari/sessions.py` (whole file - it is ~30 lines)
- Test: `tests/test_sessions.py` (append)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `register(session: str, cwd=None) -> None`
  - `set_foreground(session: str, cwd=None) -> None`
  - `unregister(session: str) -> None` (now also clears the pin if it pointed here)
  - `foreground() -> str | None` (returns `_pinned` when pinned, else `_foreground`)
  - `is_foreground(session: str) -> bool` (pin-aware)
  - `pinned() -> str | None`
  - `folder(session: str) -> str | None` (recorded cwd basename, or None)
  - `pin_toggle() -> tuple[str, str | None]` → `(action, folder)`, `action in {"pinned","unpinned","none"}`. Operates on the RAW last-prompt foreground (`_foreground`), not the pin-aware value.

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_sessions.py`:

```python
# --- cwd capture + folder name -------------------------------------------

def test_register_records_cwd_basename_posix():
    sm = SessionManager()
    sm.register("s1", cwd="/home/me/myapp")
    assert sm.folder("s1") == "myapp"


def test_set_foreground_records_cwd_basename_windows_path():
    sm = SessionManager()
    sm.set_foreground("s1", cwd="C:\\Users\\me\\proj")
    assert sm.folder("s1") == "proj"      # portable: handles backslashes on any host


def test_folder_unknown_session_is_none():
    sm = SessionManager()
    assert sm.folder("nope") is None


def test_empty_cwd_does_not_clobber_known_folder():
    sm = SessionManager()
    sm.register("s1", cwd="/x/myapp")
    sm.set_foreground("s1", cwd="")       # later message with no cwd
    assert sm.folder("s1") == "myapp"     # keep the good name


# --- pin toggle ----------------------------------------------------------

def test_pin_toggle_no_foreground_returns_none():
    sm = SessionManager()
    assert sm.pin_toggle() == ("none", None)
    assert sm.pinned() is None


def test_pin_toggle_pins_current_foreground():
    sm = SessionManager()
    sm.set_foreground("s1", cwd="/x/myapp")
    assert sm.pin_toggle() == ("pinned", "myapp")
    assert sm.pinned() == "s1"


def test_pin_toggle_same_session_again_unpins():
    sm = SessionManager()
    sm.set_foreground("s1", cwd="/x/myapp")
    sm.pin_toggle()                       # pin
    assert sm.pin_toggle() == ("unpinned", "myapp")   # toggle off
    assert sm.pinned() is None


def test_pin_holds_foreground_when_another_session_submits():
    sm = SessionManager()
    sm.set_foreground("s1", cwd="/x/a")
    sm.pin_toggle()                       # pin s1
    sm.set_foreground("s2", cwd="/x/b")   # another session submits a prompt
    assert sm.foreground() == "s1"        # pin holds
    assert sm.is_foreground("s1") is True
    assert sm.is_foreground("s2") is False


def test_pin_moves_when_toggled_while_another_is_foreground():
    sm = SessionManager()
    sm.set_foreground("s1", cwd="/x/a")
    sm.pin_toggle()                       # pin s1
    sm.set_foreground("s2", cwd="/x/b")   # s2 now last-prompt (but s1 pinned)
    # NOTE: pin_toggle acts on the RAW last-prompt foreground (s2), so it moves the pin
    assert sm.pin_toggle() == ("pinned", "b")
    assert sm.pinned() == "s2"


def test_unregister_pinned_session_falls_back_to_auto():
    sm = SessionManager()
    sm.set_foreground("s1", cwd="/x/a")
    sm.pin_toggle()                       # pin s1
    sm.unregister("s1")
    assert sm.pinned() is None
    assert sm.foreground() is None
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/test_sessions.py -q`
Expected: FAIL - `AttributeError: 'SessionManager' object has no attribute 'folder'` / `pin_toggle` / `pinned`, and `register()`/`set_foreground()` reject the `cwd=` kwarg.

- [ ] **Step 3: Rewrite `src/sonari/sessions.py`**

```python
from __future__ import annotations


def _basename(cwd) -> "str | None":
    """Portable last path component of *cwd*, handling both / and \\ separators
    regardless of host OS (a Windows cwd is named correctly even on a macOS runner).
    Empty/None -> None."""
    if not cwd:
        return None
    s = str(cwd).replace("\\", "/").rstrip("/")
    base = s.rsplit("/", 1)[-1]
    return base or None


class SessionManager:
    def __init__(self, background_policy: str = "earcon_only") -> None:
        self.background_policy = background_policy
        # session id -> cwd basename (or None). Insertion-ordered (dict) so a future
        # list/cycle is stable; membership/`in`/len behave like the old set.
        self._sessions: "dict[str, str | None]" = {}
        self._foreground: "str | None" = None
        self._pinned: "str | None" = None      # None = auto (follow last prompt)

    def _record(self, session: str, cwd) -> None:
        folder = _basename(cwd)
        if session not in self._sessions:
            self._sessions[session] = folder
        elif folder:                            # update only with a non-empty name
            self._sessions[session] = folder

    def set_foreground(self, session: str, cwd=None) -> None:
        self._record(session, cwd)
        self._foreground = session

    def foreground(self) -> "str | None":
        """The session that owns the voice: the pinned one if pinned, else the last
        session to submit a prompt / start."""
        return self._pinned if self._pinned is not None else self._foreground

    def is_foreground(self, session: str) -> bool:
        fg = self.foreground()
        return fg is not None and session == fg

    def register(self, session: str, cwd=None) -> None:
        self._record(session, cwd)

    def unregister(self, session: str) -> None:
        self._sessions.pop(session, None)
        if self._foreground == session:
            self._foreground = None
        if self._pinned == session:             # pinned session ended -> auto
            self._pinned = None

    def should_speak(self, session: str) -> bool:
        return self.is_foreground(session)

    def pinned(self) -> "str | None":
        return self._pinned

    def folder(self, session: str) -> "str | None":
        return self._sessions.get(session)

    def pin_toggle(self) -> "tuple[str, str | None]":
        """Toggle the pin against the RAW last-prompt foreground.

        - no foreground          -> ("none", None), no change
        - already pinned to it   -> unpin -> ("unpinned", folder)
        - otherwise              -> pin it -> ("pinned", folder)
        """
        cur = self._foreground
        if cur is None:
            return ("none", None)
        if self._pinned == cur:
            self._pinned = None
            return ("unpinned", self._sessions.get(cur))
        self._pinned = cur
        return ("pinned", self._sessions.get(cur))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_sessions.py -q`
Expected: PASS (all existing + new). The old tests still pass because `register`/`set_foreground` keep working with no `cwd`, and `foreground()` is unchanged when nothing is pinned.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/sessions.py tests/test_sessions.py
git commit -m "feat(core): SessionManager cwd capture + pin state (#31)"
```

---

### Task 2: protocol PIN_TOGGLE + daemon handler + cwd passthrough

**Files:**
- Modify: `src/sonari/protocol.py` (add one `MsgType` member)
- Modify: `src/sonari/daemon.py` (new `PIN_TOGGLE` branch in `handle_message`; pass `cwd` into the `SET_FOREGROUND`/`SESSION_START` handler)
- Test: `tests/test_daemon_pin.py` (create)

**Interfaces:**
- Consumes: `SessionManager.pin_toggle()`, `.foreground()`, `.pinned()` (Task 1); `self._enqueue(session, kind, text, is_decision, mute_exempt=...)`; `self.speaker.earcon(kind)`.
- Produces: handles a `{"type": "pin_toggle", "session": ...}` message; `MsgType.PIN_TOGGLE = "pin_toggle"`.

- [ ] **Step 1: Write the failing tests**

Create `tests/test_daemon_pin.py`:

```python
"""Pin-toggle hotkey: pin the current session's voice; toggle again to unpin."""
from tests.daemon_helpers import make_daemon


def test_pin_toggle_pins_current_and_speaks_folder():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    sessions.set_foreground("fg", cwd="/home/me/myapp")
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})
    assert sessions.pinned() == "fg"
    daemon._speak_loop_once()
    assert speaker.spoken == ["Pinned myapp."]


def test_pin_toggle_again_unpins_and_says_auto():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    sessions.set_foreground("fg", cwd="/home/me/myapp")
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})   # pin
    daemon._speak_loop_once()
    speaker.spoken.clear()
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})   # unpin
    assert sessions.pinned() is None
    daemon._speak_loop_once()
    assert speaker.spoken == ["Auto."]


def test_pinned_session_keeps_voice_when_another_submits():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})  # pin fg
    daemon.handle_message({"type": "set_foreground", "session": "bg"})
    assert sessions.foreground() == "fg"
    assert sessions.is_foreground("fg") is True
    assert sessions.is_foreground("bg") is False


def test_pinned_session_end_falls_back_to_auto():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})
    daemon.handle_message({"type": "session_end", "session": "fg"})
    assert sessions.pinned() is None
    assert sessions.foreground() is None


def test_set_foreground_message_carries_cwd_into_announcement():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground=None)
    daemon.handle_message({"type": "set_foreground", "session": "s1", "cwd": "/x/proj"})
    daemon.handle_message({"type": "pin_toggle", "session": "s1"})
    daemon._speak_loop_once()
    assert speaker.spoken == ["Pinned proj."]


def test_pin_toggle_with_no_session_beeps_error_only():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground=None)
    daemon.handle_message({"type": "pin_toggle", "session": ""})
    assert sessions.pinned() is None
    assert speaker.earcons[-1] == "error"
    assert speaker.spoken == []
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/test_daemon_pin.py -q`
Expected: FAIL - the `pin_toggle` message is unhandled (no announcement enqueued; `speaker.spoken` empty), and `set_foreground` with `cwd` does not reach the manager.

- [ ] **Step 3: Add `MsgType.PIN_TOGGLE`**

In `src/sonari/protocol.py`, in `class MsgType`, after the `MUTE` line (`MUTE = "mute"`), add:

```python
    PIN_TOGGLE = "pin_toggle"   # pin/unpin the voice to the current session (#31)
```

- [ ] **Step 4: Pass `cwd` through the foreground handler**

In `src/sonari/daemon.py`, change the `SET_FOREGROUND`/`SESSION_START` branch:

```python
        if t in (MsgType.SET_FOREGROUND, MsgType.SESSION_START):
            self.sessions.set_foreground(session, cwd=msg.get("cwd"))
            if t == MsgType.SESSION_START:
                self.sessions.register(session, cwd=msg.get("cwd"))
                self._maybe_guide_setup(session, msg.get("plugin_version", ""))
            return None
```

- [ ] **Step 5: Add the `PIN_TOGGLE` handler**

In `src/sonari/daemon.py`, immediately AFTER the `MsgType.MUTE` branch (the block that ends with the `Session muted.` enqueue and `return None`) and BEFORE the `MsgType.RELOAD_KEYMAP` branch, insert:

```python
        if t == MsgType.PIN_TOGGLE:
            # Pin the voice to the current (last-prompt) session, or unpin it.
            # The pin overrides "foreground", so a later SET_FOREGROUND from another
            # session can't steal the voice. Confirmation is mute_exempt so the user
            # always hears it; the no-session case has nothing to speak through, so
            # it is an error earcon only.
            action, folder = self.sessions.pin_toggle()
            if action == "none":
                self.speaker.earcon("error")
                return None
            fg = self.sessions.foreground()
            if action == "pinned":
                text = "Pinned {0}.".format(folder) if folder else "Pinned."
            else:
                text = "Auto."
            self._enqueue(fg, "prose", text, False, mute_exempt=True)
            return None
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `python -m pytest tests/test_daemon_pin.py -q`
Expected: PASS (6 tests).

- [ ] **Step 7: Commit**

```bash
git add src/sonari/protocol.py src/sonari/daemon.py tests/test_daemon_pin.py
git commit -m "feat(core): PIN_TOGGLE daemon handler + cwd passthrough (#31)"
```

---

### Task 3: hooks_entry - send cwd from the hook payload

**Files:**
- Modify: `src/sonari/hooks_entry.py` (add `cwd` to the `SET_FOREGROUND` + `SESSION_START` messages)
- Test: `tests/test_hooks_entry.py` (append)

**Interfaces:**
- Consumes: the Claude Code hook `payload` dict (already parsed; carries `cwd`).
- Produces: `SET_FOREGROUND` and `SESSION_START` messages now include `cwd=payload.get("cwd", "")`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_hooks_entry.py`:

```python
def test_user_prompt_submit_sets_foreground_with_cwd():
    msgs = handle_event("UserPromptSubmit", {"session_id": "s1", "cwd": "/x/proj"})
    fg = [m for m in msgs if m["type"] == "set_foreground"]
    assert fg and fg[0]["cwd"] == "/x/proj"


def test_session_start_carries_cwd():
    msgs = handle_event("SessionStart", {"session_id": "s1", "cwd": "/x/proj"})
    ss = [m for m in msgs if m["type"] == "session_start"]
    assert ss and ss[0]["cwd"] == "/x/proj"


def test_missing_cwd_defaults_to_empty_string():
    msgs = handle_event("UserPromptSubmit", {"session_id": "s1"})
    fg = [m for m in msgs if m["type"] == "set_foreground"]
    assert fg and fg[0]["cwd"] == ""
```

(If `handle_event` is not already imported at the top of `tests/test_hooks_entry.py`, add `from sonari.hooks_entry import handle_event`.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/test_hooks_entry.py -q`
Expected: FAIL - `KeyError: 'cwd'` (the messages have no `cwd` field yet).

- [ ] **Step 3: Add `cwd` to the messages**

In `src/sonari/hooks_entry.py`, in the `UserPromptSubmit` branch:

```python
    if event == "UserPromptSubmit":
        return [
            _msg(type=MsgType.SET_FOREGROUND, session=session,
                 cwd=payload.get("cwd", "")),
            _msg(type=MsgType.FLUSH, session=session),
        ]
```

And in the `SessionStart` branch:

```python
    if event == "SessionStart":
        return [
            _msg(type=MsgType.SET_FOREGROUND, session=session,
                 cwd=payload.get("cwd", "")),
            _msg(
                type=MsgType.SESSION_START,
                session=session,
                cwd=payload.get("cwd", ""),
                plugin_version=os.environ.get("CLAUDE_PLUGIN_VERSION", ""),
                plugin_root=os.environ.get("CLAUDE_PLUGIN_ROOT", ""),
            ),
        ]
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_hooks_entry.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/hooks_entry.py tests/test_hooks_entry.py
git commit -m "feat(core): hook sends cwd so the daemon can name the pinned folder (#31)"
```

---

### Task 4: keymap - pin_toggle action + default binding

**Files:**
- Modify: `src/sonari/keymap.py` (add to `ACTION_MESSAGES` and `_DEFAULT_KEYS`)
- Test: `tests/test_keymap.py` (append)

**Interfaces:**
- Consumes: nothing new (the Windows + macOS hotkey backends already forward `resolve_keymap`'s per-action `message` JSON generically - verified in `windows/hotkeys.py::_register_all`).
- **Correction (found in execution):** the platform key tables are *sparse* (only keys used by defaults: `s r d l v o p m`). A new key letter therefore MUST also be added to both `KEY_CODES` tables (`windows/keytables.py` `f=0x46`, `macos/keytables.py` `f=3`) and the display-label maps (`windows/hotkeys.py` `0x46:"F"`, `macos/hotkeys.py` `f:"F"`), or `resolve_keymap` raises `ValueError: unknown key`. This is required, not optional - and it means the task also touches `macos/**` (Nima's domain). Four one-line additive entries.
- Produces: `ACTION_MESSAGES["pin_toggle"] == {"type": "pin_toggle"}`; default binding key `"f"`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/test_keymap.py` (add `import json` at the top if absent):

```python
def test_pin_toggle_action_message():
    from sonari.keymap import ACTION_MESSAGES
    assert ACTION_MESSAGES["pin_toggle"] == {"type": "pin_toggle"}


def test_pin_toggle_default_binding_is_f():
    from sonari.keymap import default_keymap
    km = default_keymap()
    assert km["pin_toggle"]["key"] == "f"     # 'p' is taken by pause


def test_pin_toggle_resolves_to_its_message():
    import json
    from sonari.keymap import resolve_keymap, default_keymap
    resolved = resolve_keymap(default_keymap())
    msgs = [json.loads(e["message"]) for e in resolved]
    assert {"type": "pin_toggle"} in msgs


def test_pin_toggle_is_clearable():
    # an unknown action raises; a known one does not -> proves it is registered
    from sonari.keymap import resolve_keymap
    from sonari.platform import get_platform
    mods = list(get_platform().hotkey.default_mods())
    resolve_keymap({"pin_toggle": {"key": "", "mods": mods}})   # cleared -> no raise
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python -m pytest tests/test_keymap.py -q`
Expected: FAIL - `KeyError: 'pin_toggle'` in `ACTION_MESSAGES` / `default_keymap`.

- [ ] **Step 3: Add the action + default key**

In `src/sonari/keymap.py`, in `ACTION_MESSAGES`, after the `"mute"` entry:

```python
    "mute": {"type": "mute"},       # sticky per-session mute toggle
    "pin_toggle": {"type": "pin_toggle"},   # pin/unpin the voice to the current session (#31)
```

In `_DEFAULT_KEYS`, add `pin_toggle` bound to `f` (NOT `p` - `pause` owns `p`):

```python
    _DEFAULT_KEYS = {
        "nav_prev": "left", "nav_next": "right", "nav_first": "up", "nav_last": "down",
        "pause": "p", "mute": "m", "pin_toggle": "f",
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python -m pytest tests/test_keymap.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/sonari/keymap.py tests/test_keymap.py
git commit -m "feat(core): pin_toggle hotkey action, default chord+F (#31)"
```

---

### Task 5: Full-suite regression check + docs

**Files:**
- Modify: `docs/superpowers/specs/2026-06-18-session-pin-focus-design.md` (note the `f` default-key decision, superseding the `P` placeholder)
- Verify: whole test suite

**Interfaces:** none.

- [ ] **Step 1: Confirm nothing iterates `_sessions` as a set elsewhere**

Run: `git grep -n "_sessions" src/sonari` (outside `sessions.py`)
Expected: no external use - `_sessions` is private to `SessionManager`; all callers use public methods. If any consumer iterates it expecting a set, it still works (dict iterates keys, supports `in`/`len`). Note any finding; do not change behavior.

- [ ] **Step 2: Record the key decision in the spec**

In the spec, under "Behavior", change the parenthetical `default chord+P` to:

```
(`pin_toggle` action; default chord+`F` - `P` is taken by `pause`; see Keybind)
```

- [ ] **Step 3: Run the full suite and diff against a fresh main baseline**

```bash
# 1. fresh clean-main baseline in a throwaway worktree (run from the repo root):
git worktree add /tmp/sonari-base origin/main --detach -q
( cd /tmp/sonari-base && python -m pytest -q 2>&1 | grep -E "^FAILED" | sed 's/ -.*//' | sort ) > /tmp/base_fresh.txt
# 2. this branch's failures:
python -m pytest -q 2>&1 | grep -E "^FAILED" | sed 's/ -.*//' | sort > /tmp/pin_fails.txt
python -m pytest -q 2>&1 | grep -E "passed|failed" | tail -1
# 3. new failures introduced by this branch - MUST be empty:
comm -23 /tmp/pin_fails.txt /tmp/base_fresh.txt
git worktree remove /tmp/sonari-base --force
```
Expected: the diff in step 3 is empty (no new failures); passed count increased by the new tests.

- [ ] **Step 4: Commit the docs note**

```bash
git add docs/superpowers/specs/2026-06-18-session-pin-focus-design.md
git commit -m "docs: record chord+F default for pin_toggle (#31)"
```

---

## Out of scope (do not build)

- Cycle / list-sessions hotkeys (single `P`/`F` toggle only).
- OS window/tab auto-detection.
- A `sonari sessions` CLI listing command.
- macOS Kokoro playback.

## After the plan

- Per CONTRIBUTING: shared core + both hotkey backends → both approve; the new default keybind (`chord+F`) is a behavior change → raise with Nima before finalizing the default. Per-platform human acceptance.
- PR off `core/session-pin` into `main`, closes #31.
