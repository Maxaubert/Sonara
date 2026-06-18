# Sonari Phase 3 — Milestone 3: Windows Global Hotkeys — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Windows the same global-hotkey actions Sonari has on macOS (stop, repeat, skip, jump-to-decision, catch-up, faster, slower, cycle-verbosity, reread-options) via in-process `RegisterHotKey`, replacing the `WinHotkeyBackend` stub.

**Architecture:** Pure-Python `ctypes` `RegisterHotKey` + a `GetMessage` pump on a **daemon-owned thread** (no second process, no compiler — the Windows analogue of the macOS Swift/Carbon hotkeyd). On `WM_HOTKEY` the thread dispatches the binding's protocol message straight into the running daemon (in-process). The keymap resolver becomes OS-agnostic: per-OS key/modifier tables and the default chord move behind the platform seam. Default chord is `Ctrl+Shift+Alt+<key>` (configurable); collisions are detected (`GetLastError()==1409`) and surfaced in `doctor`.

**Tech Stack:** Python 3.9 core; stdlib `ctypes` (`user32`: `RegisterHotKey`/`UnregisterHotKey`/`GetMessage`/`PostThreadMessageW`), all lazily imported; pytest with thin monkeypatchable wrappers (no real Win32 needed for unit tests). Grounding: `docs/superpowers/specs/2026-06-10-sonari-phase3-windows-design.md` §3 (Hotkeys), §7 (locked decisions), §8 (landmines).

**Branch:** continue on `feat/windows-install-seam` (or a fresh `feat/windows-hotkeys` off it). M1/M2 + the install seam are already in place.

---

## Invariants (hold at EVERY commit)

1. **macOS behavior unchanged.** `get_platform()` returns the macOS backend on darwin; the macOS hotkeyd (separate Swift process) is untouched. The macOS keymap still resolves to the same Carbon codes and the same `Ctrl+Cmd` default chord. Run the suite each task (on this Windows box, diff against the known ~24-failure environment baseline — see `[[sonari-windows-test-running]]`; the true macOS gate runs on a Mac/CI).
2. **In-process, no toolchain.** Windows hotkeys run on a thread inside the existing daemon process. No new process, no compiler, no admin.
3. **Windows-only imports stay lazy.** Every `ctypes.windll`/`winuser` access is inside a method, never at module import, so `platform/windows/*` imports clean on macOS for the mock suite.
4. **`⚠` items are deferred to on-hardware acceptance.** Whether a chord actually fires system-wide, and the UIPI-elevation gap, cannot be unit-verified — they go in `M3-WINDOWS-ACCEPTANCE.md`, never asserted from a mock.

---

## File Structure

```
src/sonari/platform/windows/keytables.py   # NEW — Win32 VK codes + RegisterHotKey MOD masks + default chord
src/sonari/platform/macos/keytables.py     # +DEFAULT_MODS = ["ctrl","cmd"] (data only)
src/sonari/platform/base.py                # HotkeyBackend: + key_codes/mod_masks/default_mods/start/stop/doctor_rows (concrete defaults)
src/sonari/platform/macos/hotkeys.py       # implement key_codes/mod_masks/default_mods (return the macOS tables); start/stop stay no-op
src/sonari/platform/windows/hotkeys.py     # REPLACE stub — real RegisterHotKey thread, dispatch, collisions, display_combo
src/sonari/keymap.py                        # resolver reads keytables via the platform; default_keymap() per-OS chord
src/sonari/daemon.py                        # run() starts the hotkey thread w/ a dispatch cb; shutdown stops it
tests/test_win_keytables.py                 # NEW
tests/test_win_hotkeys.py                   # NEW (replaces stub coverage in test_win_backend.py)
tests/test_keymap.py                        # platform-dispatched resolution (mac + win under monkeypatch)
tests/test_daemon_hotkeys.py                # NEW — run() wires hotkey start/stop
docs/superpowers/M3-WINDOWS-ACCEPTANCE.md   # NEW — the deferred on-hardware checklist
```

---

## Task 1: Windows key/modifier tables

**Files:** Create `src/sonari/platform/windows/keytables.py`; add one line to `src/sonari/platform/macos/keytables.py`; Test `tests/test_win_keytables.py`

- [ ] **Step 1: Write the failing test**
```python
# tests/test_win_keytables.py
from sonari.platform.windows import keytables as wk

def test_vk_codes_for_default_action_keys():
    # Virtual-Key codes (Win32): letters are their ASCII uppercase ordinals.
    assert wk.KEY_CODES["s"] == 0x53 and wk.KEY_CODES["o"] == 0x4F
    assert wk.KEY_CODES["."] == 0xBE          # VK_OEM_PERIOD
    assert wk.KEY_CODES["]"] == 0xDD and wk.KEY_CODES["["] == 0xDB

def test_mod_masks_are_registerhotkey_fsmodifiers():
    assert wk.MOD_MASKS["alt"] == 0x0001 and wk.MOD_MASKS["ctrl"] == 0x0002
    assert wk.MOD_MASKS["shift"] == 0x0004 and wk.MOD_MASKS["win"] == 0x0008

def test_default_mods_is_ctrl_shift_alt():
    assert wk.DEFAULT_MODS == ["ctrl", "shift", "alt"]
```

- [ ] **Step 2: Run → FAIL** (module missing). `PYTHONPATH=src python -m pytest tests/test_win_keytables.py -q`

- [ ] **Step 3: Create `src/sonari/platform/windows/keytables.py`** (pure data, no imports):
```python
"""Win32 virtual-key codes + RegisterHotKey fsModifiers, and the Windows default
chord. Pure data — no OS calls — so it imports on any host for the mock suite."""

# Virtual-Key codes. Letters == ASCII uppercase; OEM keys per WinUser.h.
KEY_CODES = {
    "s": 0x53, "r": 0x52, "d": 0x44, "l": 0x4C, "v": 0x56, "o": 0x4F,
    "period": 0xBE, ".": 0xBE,        # VK_OEM_PERIOD
    "rightbracket": 0xDD, "]": 0xDD,  # VK_OEM_6
    "leftbracket": 0xDB, "[": 0xDB,   # VK_OEM_4
}

# RegisterHotKey fsModifiers (WinUser.h). NOT the Carbon masks.
MOD_MASKS = {
    "alt": 0x0001, "ctrl": 0x0002, "control": 0x0002,
    "shift": 0x0004, "win": 0x0008, "cmd": 0x0008,  # map 'cmd' -> Win key for portability
}

# MOD_NOREPEAT (0x4000) is OR-ed in at register time, not part of a chord.
MOD_NOREPEAT = 0x4000

# Default chord: Ctrl+Shift+Alt clears AltGr / Win-reserved / terminal / layout collisions.
DEFAULT_MODS = ["ctrl", "shift", "alt"]
```

- [ ] **Step 4: Add the macOS default chord** — append to `src/sonari/platform/macos/keytables.py` (data only, preserves existing `KEY_CODES`/`MOD_MASKS`):
```python
# Default chord on macOS (Ctrl+Cmd, avoids VoiceOver's Ctrl+Opt).
DEFAULT_MODS = ["ctrl", "cmd"]
```

- [ ] **Step 5: Run → PASS**, then full suite (no regressions vs baseline). Commit:
```bash
git add src/sonari/platform/windows/keytables.py src/sonari/platform/macos/keytables.py tests/test_win_keytables.py
git commit -m "feat(windows): Win32 VK + RegisterHotKey modifier tables + Ctrl+Shift+Alt default chord"
```

---

## Task 2: `HotkeyBackend` ABC — keytables + lifecycle hooks

**Files:** Modify `src/sonari/platform/base.py`; Modify `src/sonari/platform/macos/hotkeys.py`; Test `tests/test_platform_base.py`

> Concrete defaults so no existing subclass/double breaks; macOS overrides the keytable getters, Windows (Task 4) overrides everything.

- [ ] **Step 1: Write the failing test**
```python
# tests/test_platform_base.py (add)
def test_macos_hotkey_exposes_keytables_and_default_mods():
    from sonari.platform.macos.hotkeys import MacHotkeyBackend
    hk = MacHotkeyBackend()
    assert hk.key_codes()["s"] == 1 and hk.mod_masks()["cmd"] == 256
    assert hk.default_mods() == ["ctrl", "cmd"]

def test_base_hotkey_lifecycle_defaults_are_noops():
    from sonari.platform.macos.hotkeys import MacHotkeyBackend
    hk = MacHotkeyBackend()
    hk.start(lambda msg: None)   # macOS: hotkeyd is a separate process -> no-op
    hk.stop()
    assert hk.doctor_rows() == []
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Add concrete methods to `HotkeyBackend`** in `base.py` (after `display_combo`):
```python
    # --- keytables (consumed by the portable keymap resolver) ---
    def key_codes(self) -> "dict":
        """Map key-name -> OS key code for this platform."""
        return {}

    def mod_masks(self) -> "dict":
        """Map modifier-name -> OS modifier mask for this platform."""
        return {}

    def default_mods(self) -> "list":
        """The platform's default modifier chord (e.g. ['ctrl','cmd'])."""
        return []

    # --- in-process lifecycle (Windows runs a thread; macOS runs a process) ---
    def start(self, dispatch) -> None:
        """Begin listening for global hotkeys. *dispatch* is callable(message: dict)
        invoked on each fire. Default: no-op (macOS hotkeyd is a separate process)."""
        return None

    def stop(self) -> None:
        """Stop listening. Default: no-op."""
        return None

    def doctor_rows(self) -> "list":
        """Platform hotkey diagnostics (collisions, integrity). Default: none."""
        return []
```

- [ ] **Step 4: Implement the macOS keytable getters** in `MacHotkeyBackend` (`platform/macos/hotkeys.py`) — return the existing tables (import at method scope to avoid cycles):
```python
    def key_codes(self) -> dict:
        from sonari.platform.macos import keytables
        return dict(keytables.KEY_CODES)

    def mod_masks(self) -> dict:
        from sonari.platform.macos import keytables
        return dict(keytables.MOD_MASKS)

    def default_mods(self) -> list:
        from sonari.platform.macos import keytables
        return list(keytables.DEFAULT_MODS)
```

- [ ] **Step 5: Run → PASS**, full suite, commit:
```bash
git add src/sonari/platform/base.py src/sonari/platform/macos/hotkeys.py tests/test_platform_base.py
git commit -m "feat(platform): HotkeyBackend keytable getters + start/stop/doctor_rows lifecycle hooks"
```

---

## Task 3: Portable keymap resolver (platform-dispatched keytables + per-OS chord)

**Files:** Modify `src/sonari/keymap.py`; Modify `tests/test_keymap.py`

> Removes the hardcoded macOS import (`keymap.py:26`). The resolver pulls key/mod tables and the default chord from `get_platform().hotkey` at call time (lazy — no import-time OS dispatch). macOS output is unchanged because the macOS getters return the same tables + `Ctrl+Cmd`.

- [ ] **Step 1: Write the failing tests** (both OSes via the factory monkeypatch)
```python
# tests/test_keymap.py (add; keep existing macOS-default tests but route through default_keymap())
def test_default_keymap_uses_platform_default_mods(monkeypatch):
    import sonari.keymap as km
    # Windows backend -> Ctrl+Shift+Alt default chord.
    import sonari.platform as platform
    monkeypatch.setattr(platform.sys, "platform", "win32")
    platform._CACHE = None
    d = km.default_keymap()
    assert d["stop"]["mods"] == ["ctrl", "shift", "alt"]
    platform._CACHE = None

def test_resolve_uses_windows_vk_codes(monkeypatch):
    import sonari.keymap as km
    import sonari.platform as platform
    monkeypatch.setattr(platform.sys, "platform", "win32")
    platform._CACHE = None
    resolved = km.resolve_keymap({"stop": {"key": "s", "mods": ["ctrl", "shift", "alt"]}})
    row = resolved[0]
    assert row["keyCode"] == 0x53                        # VK 'S'
    assert row["modifiers"] == (0x0002 | 0x0004 | 0x0001)  # ctrl|shift|alt
    assert row["action"] == "stop"
    platform._CACHE = None
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Rewrite the table source + default in `keymap.py`.** Delete the `from sonari.platform.macos.keytables import KEY_CODES, MOD_MASKS` line (and its comment block). Replace the `DEFAULT_KEYMAP` constant + `resolve_keymap`'s table use:
```python
# --- shared action -> key (the chord modifiers are platform-defaulted) ---
_DEFAULT_KEYS = {
    "stop": "s", "repeat": "r", "skip": ".", "jump_decision": "d",
    "catch_up": "l", "faster": "]", "slower": "[",
    "cycle_verbosity": "v", "reread_options": "o",
}


def _keytables():
    """(key_codes, mod_masks) for the active platform (lazy — no import-time dispatch)."""
    from sonari.platform import get_platform
    hk = get_platform().hotkey
    return hk.key_codes(), hk.mod_masks()


def default_keymap() -> dict:
    """The default action->binding map for the active platform (per-OS chord)."""
    from sonari.platform import get_platform
    mods = get_platform().hotkey.default_mods()
    return {action: {"key": key, "mods": list(mods)}
            for action, key in _DEFAULT_KEYS.items()}
```
In `resolve_keymap`, replace `if keymap is None: keymap = DEFAULT_KEYMAP` with `if keymap is None: keymap = default_keymap()`, and at the top of the loop fetch tables once: `key_codes, mod_masks = _keytables()`, then use `key_codes`/`mod_masks` in place of the old `KEY_CODES`/`MOD_MASKS`. In `load_keymap` and `write_default_keymap_if_absent`, replace `_copy_keymap(DEFAULT_KEYMAP)` / `DEFAULT_KEYMAP` with `default_keymap()`.

- [ ] **Step 4: Update `tests/test_keymap.py`** existing macOS-default assertions to call `km.default_keymap()` under the default (darwin) factory, and to read `km._DEFAULT_KEYS` where they referenced the old `DEFAULT_KEYMAP` keys. (Mac path: `default_mods()` → `['ctrl','cmd']`, so the macOS default keymap is byte-identical to the old `DEFAULT_KEYMAP`.)

- [ ] **Step 5: Run → PASS** (mac + win paths), full suite, commit:
```bash
git add src/sonari/keymap.py tests/test_keymap.py
git commit -m "refactor(keymap): resolve key/mod tables + default chord via the platform seam (no hardcoded macOS import)"
```

---

## Task 4: `WinHotkeyBackend` — real RegisterHotKey thread

**Files:** Replace `src/sonari/platform/windows/hotkeys.py`; Test `tests/test_win_hotkeys.py`

> Thin monkeypatchable wrappers around `user32` (`_register`/`_unregister`/`_get_message`/`_post_quit`/`_last_error`) so the registration + dispatch logic is unit-testable on macOS with no real Win32 (mirrors `WinSupervisorBackend._schtasks`). The real message pump is exercised only on-hardware (Task 7).

- [ ] **Step 1: Write the failing tests**
```python
# tests/test_win_hotkeys.py
from sonari.platform.windows.hotkeys import WinHotkeyBackend
from sonari.platform.base import HotkeyBackend

def _backend_with_fake_user32(monkeypatch, *, fail_ids=()):
    hk = WinHotkeyBackend()
    registered = []
    def _register(hid, mods, vk):
        if hid in fail_ids:
            return 0           # RegisterHotKey FALSE
        registered.append((hid, mods, vk)); return 1
    monkeypatch.setattr(hk, "_register", _register)
    monkeypatch.setattr(hk, "_unregister", lambda hid: 1)
    monkeypatch.setattr(hk, "_last_error", lambda: 1409)  # ERROR_HOTKEY_ALREADY_REGISTERED
    return hk, registered

def test_keytables_and_default_mods():
    hk = WinHotkeyBackend()
    assert hk.key_codes()["s"] == 0x53 and hk.mod_masks()["ctrl"] == 0x0002
    assert hk.default_mods() == ["ctrl", "shift", "alt"]
    assert isinstance(hk, HotkeyBackend)

def test_register_bindings_maps_ids_to_messages(monkeypatch):
    hk, registered = _backend_with_fake_user32(monkeypatch)
    resolved = [{"action": "stop", "keyCode": 0x53,
                 "modifiers": 0x0002 | 0x0004 | 0x0001,
                 "message": '{"type": "stop"}'}]
    id_to_msg = hk._register_all(resolved)
    assert len(registered) == 1
    hid, mods, vk = registered[0]
    assert vk == 0x53 and mods == (0x0002 | 0x0004 | 0x0001 | 0x4000)  # +MOD_NOREPEAT
    assert id_to_msg[hid] == {"type": "stop"}
    assert hk.collisions == []

def test_register_records_collision_on_1409(monkeypatch):
    hk, _ = _backend_with_fake_user32(monkeypatch, fail_ids={1})  # first id collides
    resolved = [{"action": "stop", "keyCode": 0x53, "modifiers": 0x2,
                 "message": '{"type": "stop"}'}]
    hk._register_all(resolved)
    assert hk.collisions and hk.collisions[0]["action"] == "stop"

def test_dispatch_on_hotkey_id_calls_back(monkeypatch):
    hk, _ = _backend_with_fake_user32(monkeypatch)
    got = []
    id_to_msg = {5: {"type": "skip"}}
    hk._on_hotkey(5, id_to_msg, got.append)
    assert got == [{"type": "skip"}]

def test_doctor_rows_report_collisions(monkeypatch):
    hk, _ = _backend_with_fake_user32(monkeypatch, fail_ids={1})
    hk._register_all([{"action": "stop", "keyCode": 0x53, "modifiers": 0x2,
                       "message": '{"type": "stop"}'}])
    names = {r[0] for r in hk.doctor_rows()}
    assert "hotkey chords" in names

def test_display_combo_labels():
    hk = WinHotkeyBackend()
    # Ctrl|Shift|Alt + 'O'
    assert hk.display_combo(0x0002 | 0x0004 | 0x0001, 0x4F) == "Ctrl+Shift+Alt+O"
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Replace `src/sonari/platform/windows/hotkeys.py`** with the real backend. All `ctypes` use is lazy and isolated in the wrapper methods:
```python
"""Windows hotkey backend — in-process RegisterHotKey + GetMessage pump.

WINDOWS-only ctypes is reached ONLY through the _register/_unregister/_get_message/
_post_quit/_last_error wrappers (monkeypatched in tests), so this module imports
on any host. Whether a chord actually fires system-wide is on-hardware-only
(M3-WINDOWS-ACCEPTANCE.md)."""
from __future__ import annotations

import json
import threading
from typing import Optional

from sonari.platform.base import HotkeyBackend
from sonari.platform.windows import keytables

_WM_HOTKEY = 0x0312
_WM_QUIT = 0x0012
_ERROR_HOTKEY_ALREADY_REGISTERED = 1409

_VK_LABELS = {
    0x53: "S", 0x52: "R", 0x44: "D", 0x4C: "L", 0x56: "V", 0x4F: "O",
    0xBE: ".", 0xDD: "]", 0xDB: "[", 0x20: "Space", 0x0D: "Enter", 0x1B: "Escape",
}
_MOD_LABELS = [(0x0002, "Ctrl"), (0x0004, "Shift"), (0x0001, "Alt"), (0x0008, "Win")]


class WinHotkeyBackend(HotkeyBackend):
    def __init__(self) -> None:
        self.collisions: list = []
        self._thread: Optional[threading.Thread] = None
        self._thread_id: Optional[int] = None
        self._stop = threading.Event()

    # --- keytables ---
    def key_codes(self) -> dict:
        return dict(keytables.KEY_CODES)

    def mod_masks(self) -> dict:
        return dict(keytables.MOD_MASKS)

    def default_mods(self) -> list:
        return list(keytables.DEFAULT_MODS)

    # --- monkeypatchable user32 wrappers (lazy ctypes) ---
    def _register(self, hid: int, mods: int, vk: int) -> int:
        import ctypes
        return ctypes.windll.user32.RegisterHotKey(None, hid, mods, vk)

    def _unregister(self, hid: int) -> int:
        import ctypes
        return ctypes.windll.user32.UnregisterHotKey(None, hid)

    def _last_error(self) -> int:
        import ctypes
        return ctypes.windll.kernel32.GetLastError()

    def _get_message(self):
        """Block for the next thread message; return (wm, wparam, lparam) or None on WM_QUIT."""
        import ctypes
        from ctypes import wintypes
        msg = wintypes.MSG()
        r = ctypes.windll.user32.GetMessageW(ctypes.byref(msg), None, 0, 0)
        if r in (0, -1):            # 0 == WM_QUIT, -1 == error
            return None
        return (msg.message, int(msg.wParam), int(msg.lParam))

    def _post_quit(self) -> None:
        import ctypes
        if self._thread_id is not None:
            ctypes.windll.user32.PostThreadMessageW(self._thread_id, _WM_QUIT, 0, 0)

    # --- registration + dispatch logic (pure, unit-tested) ---
    def _register_all(self, resolved: list) -> dict:
        """RegisterHotKey each resolved binding (MOD_NOREPEAT added). Returns
        {hotkey_id: message_dict}. Records collisions instead of raising."""
        self.collisions = []
        id_to_msg = {}
        for i, b in enumerate(resolved, start=1):
            mods = b["modifiers"] | keytables.MOD_NOREPEAT
            if self._register(i, mods, b["keyCode"]):
                id_to_msg[i] = json.loads(b["message"])
            else:
                err = self._last_error()
                self.collisions.append({
                    "action": b["action"], "error": err,
                    "already_owned": err == _ERROR_HOTKEY_ALREADY_REGISTERED,
                })
        return id_to_msg

    def _on_hotkey(self, hid: int, id_to_msg: dict, dispatch) -> None:
        msg = id_to_msg.get(hid)
        if msg is not None:
            dispatch(msg)

    # --- lifecycle ---
    def start(self, dispatch) -> None:
        """Start a daemon thread that registers the current keymap and pumps
        WM_HOTKEY into *dispatch*. RegisterHotKey + GetMessage MUST share the same
        thread, so registration happens inside the thread."""
        from sonari import keymap
        resolved = keymap.resolve_keymap(keymap.load_keymap())
        self._stop.clear()

        def _run():
            import ctypes
            self._thread_id = ctypes.windll.kernel32.GetCurrentThreadId()
            id_to_msg = self._register_all(resolved)
            try:
                while not self._stop.is_set():
                    got = self._get_message()
                    if got is None:
                        break
                    wm, wparam, _ = got
                    if wm == _WM_HOTKEY:
                        self._on_hotkey(wparam, id_to_msg, dispatch)
            finally:
                for hid in id_to_msg:
                    self._unregister(hid)

        self._thread = threading.Thread(target=_run, name="sonari-hotkeys", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._post_quit()

    # --- diagnostics ---
    def doctor_rows(self) -> list:
        if not self.collisions:
            return [("hotkey chords", True, "no collisions")]
        owned = ", ".join(c["action"] for c in self.collisions)
        return [("hotkey chords", False,
                 "chord already owned by another app for: {0} "
                 "(rebind in ~/.sonari/keymap.json)".format(owned))]

    def display_combo(self, modifiers: int, key_code: int) -> str:
        parts = [name for mask, name in _MOD_LABELS if modifiers & mask]
        parts.append(_VK_LABELS.get(key_code, "key{0}".format(key_code)))
        return "+".join(parts)

    def uninstall(self) -> None:
        self.stop()

    def install(self, log_path: str, agent_path, launchctl_fn) -> tuple:
        # Windows hotkeys are started by the daemon (start()), not by `sonari install`.
        return (True, "Windows hotkeys run in-process; started by the daemon.")
```

- [ ] **Step 4: Run → PASS**, full suite, commit:
```bash
git add src/sonari/platform/windows/hotkeys.py tests/test_win_hotkeys.py
git commit -m "feat(windows): WinHotkeyBackend — in-process RegisterHotKey thread, dispatch, collision detection"
```

> **Note for Task 7 / test_win_backend.py:** the old `test_hotkey_stub_reports_deferred` (asserts `install()` returns `(False, ...M3...)`) must be updated/removed — `install()` now returns `(True, ...)`. Fix it in this task's Step 3 commit or Task 7.

---

## Task 5: Daemon wires the hotkey thread (start on run, stop on shutdown)

**Files:** Modify `src/sonari/daemon.py`; Test `tests/test_daemon_hotkeys.py`

> The daemon owns the hotkey thread on Windows; on macOS `start()/stop()` are no-ops (hotkeyd is a separate process), so this is safe on both. The dispatch callback feeds a hotkey's protocol message into the **same** handler path as a socket message.

- [ ] **Step 1: Write the failing test**
```python
# tests/test_daemon_hotkeys.py
def test_run_starts_and_dispatch_routes_message(monkeypatch):
    import sonari.daemon as d
    started = {}
    class FakeHotkey:
        def start(self, dispatch): started["dispatch"] = dispatch
        def stop(self): started["stopped"] = True
    class FakePlatform:
        hotkey = FakeHotkey()
    monkeypatch.setattr("sonari.platform.get_platform", lambda: FakePlatform())
    daemon = d.Daemon(queue=_FakeQueue(), speaker=_FakeSpeaker(),
                      sessions=_FakeSessions(), config={})
    daemon._start_hotkeys()                      # extracted from run()
    assert "dispatch" in started
    # Dispatching a hotkey message hits the same handler as a socket message.
    handled = []
    monkeypatch.setattr(daemon, "_handle_message", lambda m, session=None: handled.append(m))
    started["dispatch"]({"type": "skip"})
    assert handled == [{"type": "skip"}]
    daemon._stop_hotkeys()
    assert started.get("stopped") is True
```
(Provide minimal `_FakeQueue/_FakeSpeaker/_FakeSessions` stubs at the top of the test matching `Daemon.__init__`'s attribute use, mirroring the existing `tests/daemon_helpers.py` fakes — import those if available.)

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement in `daemon.py`.** Add a dispatch helper + start/stop, and call them from `run()`. First confirm the existing per-message handler name (the method `_handle_conn` calls to act on a parsed `msg`); reuse it as `_handle_message`. If the handling is inline in `_handle_conn`, extract the per-message body into `_handle_message(self, msg, session=None)` and call it from both `_handle_conn` and the hotkey dispatch. Then:
```python
    def _start_hotkeys(self) -> None:
        from sonari.platform import get_platform
        get_platform().hotkey.start(self._dispatch_hotkey)

    def _stop_hotkeys(self) -> None:
        from sonari.platform import get_platform
        try:
            get_platform().hotkey.stop()
        except Exception:  # noqa: BLE001 - shutdown must not raise
            pass

    def _dispatch_hotkey(self, message: dict) -> None:
        # A hotkey fire is handled exactly like an inbound socket message; the
        # foreground session is the daemon's current one.
        try:
            self._handle_message(message)
        except Exception:  # noqa: BLE001 - one bad hotkey must not kill the pump
            pass
```
In `run()`, after the speak/accept threads start, add `self._start_hotkeys()`, and in the shutdown path (where `_running` clears / threads join) add `self._stop_hotkeys()`.

- [ ] **Step 4: Run → PASS**, full suite (confirm `test_daemon_*` still green on the mock harness — `get_platform()` is patched), commit:
```bash
git add src/sonari/daemon.py tests/test_daemon_hotkeys.py
git commit -m "feat(daemon): start/stop the in-process hotkey thread; route fires through the shared message handler"
```

---

## Task 6: Doctor — surface hotkey collisions + the UIPI elevation warning

**Files:** Modify `src/sonari/cli.py` (`doctor()`); Modify `src/sonari/platform/windows/hotkeys.py` (integrity check); Test `tests/test_cli_doctor.py`, `tests/test_win_hotkeys.py`

> `cli.doctor()` already composes `supervisor.doctor_rows()` + neutral rows. Add `_platform().hotkey.doctor_rows()` so Windows hotkey collisions/UIPI show up; macOS hotkey `doctor_rows()` is `[]` (its rows live in the supervisor), so no macOS change.

- [ ] **Step 1: Write the failing tests**
```python
# tests/test_cli_doctor.py (add) — fake hotkey contributes a row
def test_doctor_includes_hotkey_rows(monkeypatch):
    from tests._fakeplatform import fake_platform, FakeSupervisor
    import sonari.cli as cli
    class HK:
        def doctor_rows(self): return [("hotkey chords", True, "no collisions")]
    pb = fake_platform(supervisor=FakeSupervisor())
    pb.hotkey = HK()
    monkeypatch.setattr(cli, "_platform", lambda: pb)
    monkeypatch.setattr("os.access", return_value if False else (lambda *a, **k: True))
    monkeypatch.setattr("sonari.paths.ensure_sonari_dir", lambda: None)
    monkeypatch.setattr("sonari.client.send", lambda *a, **k: {"ok": True})
    monkeypatch.setattr(cli, "_read_install_record", lambda: {"app_path": "/a"})
    monkeypatch.setattr("os.path.exists", lambda p: True)
    names = {r[0] for r in cli.doctor()}
    assert "hotkey chords" in names
```
```python
# tests/test_win_hotkeys.py (add)
def test_uipi_row_when_elevated(monkeypatch):
    from sonari.platform.windows.hotkeys import WinHotkeyBackend
    hk = WinHotkeyBackend()
    monkeypatch.setattr(hk, "_process_is_elevated", lambda: True)
    rows = hk.doctor_rows()
    assert any("Administrator" in r[2] for r in rows)
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement.** In `cli.doctor()`, after the hooks row (or near the platform rows), add:
```python
    results.extend(_platform().hotkey.doctor_rows())
```
In `WinHotkeyBackend`, add an elevation probe + a UIPI row (prepend to `doctor_rows`):
```python
    def _process_is_elevated(self) -> bool:
        """True if THIS (daemon) process runs elevated. Lazy ctypes; never raises."""
        try:
            import ctypes
            return bool(ctypes.windll.shell32.IsUserAnAdmin())
        except Exception:
            return False
```
At the top of `doctor_rows()`:
```python
        rows = []
        if self._process_is_elevated():
            rows.append(("hotkey integrity", False,
                         "daemon is elevated; hotkeys won't reach a non-elevated "
                         "Claude window. Don't run as Administrator (UIPI)."))
        # ... then the existing collisions/no-collisions row appended to rows ...
        return rows
```
(Refactor `doctor_rows` to build the `rows` list including both the UIPI row and the collisions row.)

- [ ] **Step 4: Run → PASS**, full suite, commit:
```bash
git add src/sonari/cli.py src/sonari/platform/windows/hotkeys.py tests/test_cli_doctor.py tests/test_win_hotkeys.py
git commit -m "feat(windows): doctor surfaces hotkey collisions + the UIPI elevation warning"
```

---

## Task 7: Update the stub test + the deferred on-hardware acceptance checklist

**Files:** Modify `tests/test_win_backend.py`; Create `docs/superpowers/M3-WINDOWS-ACCEPTANCE.md`

- [ ] **Step 1: Fix `test_win_backend.py`.** Replace `test_hotkey_stub_reports_deferred` (the stub returned `(False, "...M3...")`) with an assertion that the backend is now real:
```python
def test_hotkey_backend_is_real_not_stub():
    from sonari.platform.windows.hotkeys import WinHotkeyBackend
    hk = WinHotkeyBackend()
    assert hk.default_mods() == ["ctrl", "shift", "alt"]
    assert hk.display_combo(0x0002, 0x53) == "Ctrl+S"
```
Run the win-backend tests → PASS.

- [ ] **Step 2: Author `docs/superpowers/M3-WINDOWS-ACCEPTANCE.md`** — the ⚠ on-hardware checks a mock cannot prove, each with exact steps + what to observe:
  - ⚠ Each of the 9 actions fires while a **non-elevated** terminal/Claude window has focus, default `Ctrl+Shift+Alt+<key>` (stop/repeat/skip/jump/catch-up/faster/slower/cycle-verbosity/reread-options).
  - ⚠ Fires **mid-speech** (stop cuts the current utterance within ~100ms).
  - ⚠ Rebinding in `~/.sonari/keymap.json` takes effect after a daemon restart.
  - ⚠ **Collision**: pre-register `Ctrl+Shift+Alt+S` in another app (e.g. AutoHotkey) → `sonari doctor` reports the collision for `stop`, other chords still work.
  - ⚠ **UIPI**: run Claude **as Administrator** → hotkeys don't reach it; `sonari doctor` shows the integrity warning; speech still works. Run non-elevated → hotkeys work.
  - ⚠ **Secure desktop** (UAC prompt / Ctrl+Alt+Del): hotkeys + TTS silent during it, resume after (document, not a blocker).
  - Note: the GetMessage pump + actual WM_HOTKEY delivery are unverifiable from the mock suite (only the register/dispatch logic is).

- [ ] **Step 3: Commit:**
```bash
git add tests/test_win_backend.py docs/superpowers/M3-WINDOWS-ACCEPTANCE.md
git commit -m "test+docs(windows): real hotkey backend assertion + M3 on-hardware acceptance checklist"
```

---

## Self-Review (completed)

- **Spec §3 (Hotkeys) coverage:** in-process `RegisterHotKey` + `GetMessage` pump on a daemon thread → T4/T5; `Ctrl+Shift+Alt` default, configurable → T1/T3; collision detection (`GetLastError()==1409`) → T4; `doctor` reports collisions → T6; UIPI elevation warning (§8 landmine #1) → T6; keytables move behind the seam, resolver stays portable (§2) → T2/T3; no second process / no toolchain → T4/T5. Out of scope (later): per-action rebinding UI, the "speak through NVDA" mode.
- **Type consistency:** `key_codes()/mod_masks()/default_mods()/start(dispatch)/stop()/doctor_rows()` defined on the ABC (T2), implemented by both backends (T2 macOS getters, T4 Windows full). `_register_all(resolved)->dict`, `_on_hotkey(hid,id_to_msg,dispatch)`, `_register/_unregister/_get_message/_post_quit/_last_error` consistent T4↔tests. `default_keymap()`/`_DEFAULT_KEYS`/`_keytables()` consistent T3↔T4 (`start()` calls `keymap.resolve_keymap(load_keymap())`). `daemon._handle_message(msg, session=None)` reused by `_handle_conn` + `_dispatch_hotkey` (T5).
- **macOS preservation:** macOS hotkeyd path untouched; `default_mods()`=`['ctrl','cmd']` and the macОS keytables are unchanged, so `default_keymap()`/`resolve_keymap()` produce the same Carbon output; `start()/stop()` no-op on macOS, so the daemon wiring (T5) is inert there.
- **Ordering:** T1 data → T2 ABC hooks → T3 portable resolver → T4 Windows backend → T5 daemon wiring → T6 doctor → T7 stub-test + acceptance. Suite stays green (vs the Windows env baseline) throughout.
- **No placeholders:** load-bearing code (keytables, resolver dispatch, the RegisterHotKey thread + dispatch, daemon wiring, doctor rows) is inlined; the GetMessage real-firing is explicitly a deferred on-hardware item.
