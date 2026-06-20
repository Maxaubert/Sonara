"""Windows hotkey backend — in-process RegisterHotKey + GetMessage pump.

WINDOWS-only ctypes is reached ONLY through the _register/_unregister/
_get_message/_post_quit/_last_error wrappers (monkeypatched in tests), so this
module imports on any host. Whether a chord actually fires system-wide is
on-hardware-only (M3-WINDOWS-ACCEPTANCE.md)."""
from __future__ import annotations

import json
import os
import threading
from typing import Optional

from sonara.platform.base import HotkeyBackend
from sonara.platform.windows import keytables

_WM_HOTKEY = 0x0312
_WM_QUIT = 0x0012
_ERROR_HOTKEY_ALREADY_REGISTERED = 1409

_VK_LABELS = {
    0x53: "S", 0x52: "R", 0x44: "D", 0x4C: "L", 0x56: "V", 0x4F: "O",
    0x46: "F", 0x50: "P", 0x4D: "M",
    0xBE: ".", 0xDD: "]", 0xDB: "[", 0x20: "Space", 0x0D: "Enter", 0x1B: "Escape",
    0x25: "Left", 0x26: "Up", 0x27: "Right", 0x28: "Down",
}
_MOD_LABELS = [(0x0002, "Ctrl"), (0x0004, "Shift"), (0x0001, "Alt"), (0x0008, "Win")]


class WinHotkeyBackend(HotkeyBackend):
    """In-process global hotkeys via Win32 RegisterHotKey on a daemon thread."""

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

    # --- monkeypatchable user32/kernel32 wrappers (lazy ctypes) ---
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
        """Block for the next thread message; return (wm, wparam, lparam) or None
        on WM_QUIT/error."""
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

    def _process_is_elevated(self) -> bool:
        """True if THIS (daemon) process runs elevated. Lazy ctypes; never raises."""
        try:
            import ctypes
            return bool(ctypes.windll.shell32.IsUserAnAdmin())
        except Exception:
            return False

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
        from sonara import keymap
        resolved = keymap.resolve_keymap(keymap.load_keymap())
        self._stop.clear()

        def _run():
            import ctypes
            self._thread_id = ctypes.windll.kernel32.GetCurrentThreadId()
            id_to_msg = self._register_all(resolved)
            self._write_state()   # persist daemon-side diagnostics for doctor (#9)
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
                self._clear_state()   # gone -> doctor reports "daemon not running"

        self._thread = threading.Thread(target=_run, name="sonara-hotkeys", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        # JOIN the pump thread before returning. The thread's finally clause
        # unregisters every chord; without the join, a reload's immediate start()
        # re-registers the SAME chords while the old thread still owns them
        # (RegisterHotKey -> 1409), they get dropped, then the old thread's finally
        # unregisters them — leaving ALL hotkeys dark until a daemon restart (H2).
        self._stop.set()
        self._post_quit()
        t = self._thread
        if t is not None and t.is_alive():
            t.join(timeout=2.0)
        self._thread = None

    # --- daemon-side state (so `sonara doctor`, a separate process that never
    #     start()ed, reports the REAL collisions/elevation, not a fresh backend's
    #     empty/foreign state). (#9) ---
    def _state_path(self) -> str:
        from sonara import paths
        return os.path.join(str(paths.SONARA_DIR), "hotkeys.state.json")

    def _write_state(self) -> None:
        try:
            from sonara import paths
            paths.ensure_sonara_dir()
            with open(self._state_path(), "w", encoding="utf-8") as fh:
                json.dump({"collisions": self.collisions,
                           "elevated": self._process_is_elevated()}, fh)
        except Exception:  # noqa: BLE001 - diagnostics must never break start()
            pass

    def _clear_state(self) -> None:
        try:
            os.unlink(self._state_path())
        except OSError:
            pass

    def _read_state(self):
        try:
            with open(self._state_path(), "r", encoding="utf-8") as fh:
                return json.load(fh)
        except Exception:  # noqa: BLE001 - absent/unreadable -> unknown
            return None

    # --- diagnostics ---
    def doctor_rows(self) -> list:
        state = self._read_state()
        if state is None:
            # No daemon-side state: the chords live in the daemon process, which
            # isn't running (or hasn't started hotkeys). Don't assert a green
            # "no collisions" we cannot actually observe from here. (#9)
            return [("hotkey chords", True,
                     "daemon not running; start it, then re-run doctor to check "
                     "chord collisions")]
        rows = []
        if state.get("elevated"):
            rows.append(("hotkey integrity", False,
                         "daemon is elevated; hotkeys won't reach a non-elevated "
                         "Claude window. Don't run as Administrator (UIPI)."))
        collisions = state.get("collisions") or []
        if collisions:
            owned = ", ".join(c.get("action", "?") for c in collisions)
            rows.append(("hotkey chords", False,
                         "chord already owned by another app for: {0} "
                         "(rebind in ~/.sonara/keymap.json)".format(owned)))
        else:
            rows.append(("hotkey chords", True, "no collisions (daemon-reported)"))
        return rows

    def display_combo(self, modifiers: int, key_code: int) -> str:
        parts = [name for mask, name in _MOD_LABELS if modifiers & mask]
        parts.append(_VK_LABELS.get(key_code, "key{0}".format(key_code)))
        return "+".join(parts)

    def uninstall(self) -> None:
        self.stop()

    def install(self, log_path: str, agent_path, launchctl_fn) -> tuple:
        # Windows hotkeys are started by the daemon (start()), not `sonara install`.
        return (True, "Windows hotkeys run in-process; started by the daemon.")
