"""macOS hotkey backend — compiles + supervises the Swift Carbon hotkeyd."""
from __future__ import annotations

import hashlib
import os
import shutil
import subprocess

from sonari import paths
from sonari.platform.base import HotkeyBackend

LAUNCH_AGENT_LABEL = "com.sonari.hotkeyd"
LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.sonari.hotkeyd.plist")

_KEYCODE_DISPLAY = {1: "S", 15: "R", 2: "D", 37: "L", 9: "V", 31: "O",
                    47: ".", 30: "]", 33: "["}
_MOD_DISPLAY = [(4096, "Ctrl"), (256, "Cmd"), (2048, "Opt"), (512, "Shift")]


class MacHotkeyBackend(HotkeyBackend):
    _keycode_display = _KEYCODE_DISPLAY
    _mod_display = _MOD_DISPLAY

    def display_combo(self, modifiers: int, key_code: int) -> str:
        parts = [name for mask, name in self._mod_display if modifiers & mask]
        parts.append(self._keycode_display.get(key_code, "key{0}".format(key_code)))
        return "+".join(parts)

    def build(self):
        """Compile sonari-hotkeyd if swiftc is present and the source changed.
        Returns (ok: bool, detail: str). (Verbatim move of cli._build_hotkeyd.)"""
        if shutil.which("swiftc") is None:
            return (False, "swiftc not found")
        src = os.path.join(paths.repo_root(), "hotkeyd", "sonari-hotkeyd.swift")
        try:
            with open(src, "rb") as fh:
                src_hash = hashlib.sha256(fh.read()).hexdigest()
        except OSError as exc:
            return (False, "cannot read hotkeyd source: {0}".format(exc))
        hash_path = str(paths.SONARI_DIR / ".hotkeyd.srchash")
        if os.path.exists(str(paths.HOTKEYD_BIN_PATH)):
            try:
                with open(hash_path, "r", encoding="utf-8") as fh:
                    if fh.read().strip() == src_hash:
                        return (True, "{0} (unchanged; kept to preserve any "
                                "permission grants)".format(paths.HOTKEYD_BIN_PATH))
            except OSError:
                pass
        rc = subprocess.call(["swiftc", src, "-o", str(paths.HOTKEYD_BIN_PATH)])
        if rc == 0:
            try:
                with open(hash_path, "w", encoding="utf-8") as fh:
                    fh.write(src_hash)
            except OSError:
                pass
            return (True, str(paths.HOTKEYD_BIN_PATH))
        return (False, "swiftc exited {0}".format(rc))

    def install(self):
        return self.build()

    def uninstall(self) -> None:
        try:
            os.remove(str(paths.HOTKEYD_BIN_PATH))
        except OSError:
            pass
