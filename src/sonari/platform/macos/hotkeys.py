"""macOS hotkey backend — compiles + supervises the Swift Carbon hotkeyd."""
from __future__ import annotations

import hashlib
import os
import shutil
import subprocess

from sonari import paths
from sonari.platform.base import HotkeyBackend
from sonari.platform.macos import keytables

LAUNCH_AGENT_LABEL = "com.sonari.hotkeyd"
LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.sonari.hotkeyd.plist")

# --- Display tables derived from keytables so a new key/modifier added there
#     is automatically covered here (no second hand-maintained copy). ---

# Canonical display label for each key name in keytables.KEY_CODES.
# Single-letter keys → uppercase; symbolic names → their symbol.
_KEY_DISPLAY_BY_NAME = {
    "s": "S", "r": "R", "d": "D", "l": "L", "v": "V", "o": "O",
    "period": ".", ".": ".",
    "rightbracket": "]", "]": "]",
    "leftbracket": "[", "[": "[",
}

# Canonical display label + ordering for each canonical modifier name.
_MOD_DISPLAY_ORDER = [
    ("ctrl", "Ctrl"),
    ("cmd", "Cmd"),
    ("opt", "Opt"),
    ("shift", "Shift"),
]

# Build the final display dicts from keytables so the source of truth for
# which codes are valid lives in exactly one place.
_KEYCODE_DISPLAY: "dict[int, str]" = {}
for _name, _display in _KEY_DISPLAY_BY_NAME.items():
    _code = keytables.KEY_CODES.get(_name)
    if _code is not None:
        _KEYCODE_DISPLAY.setdefault(_code, _display)

_MOD_DISPLAY: "list[tuple[int, str]]" = []
_seen_mod_masks: "set[int]" = set()
for _mod_name, _mod_label in _MOD_DISPLAY_ORDER:
    _mask = keytables.MOD_MASKS.get(_mod_name)
    if _mask is not None and _mask not in _seen_mod_masks:
        _MOD_DISPLAY.append((_mask, _mod_label))
        _seen_mod_masks.add(_mask)
del _name, _display, _code, _mod_name, _mod_label, _mask, _seen_mod_masks


def _xml_escape(s: str) -> str:
    """Escape the three XML-significant characters for safe plist interpolation."""
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _hotkeyd_plist(binary_path: str, log_path: str) -> str:
    """Return the full LaunchAgent plist XML for the hotkey daemon."""
    args_xml = "        <string>{0}</string>\n".format(_xml_escape(binary_path))
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
        '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
        '<plist version="1.0">\n'
        '<dict>\n'
        '    <key>Label</key>\n'
        '    <string>{label}</string>\n'
        '    <key>ProgramArguments</key>\n'
        '    <array>\n'
        '{args}'
        '    </array>\n'
        '    <key>RunAtLoad</key>\n'
        '    <true/>\n'
        '    <key>KeepAlive</key>\n'
        '    <true/>\n'
        '    <key>StandardErrorPath</key>\n'
        '    <string>{log}</string>\n'
        '    <key>StandardOutPath</key>\n'
        '    <string>{log}</string>\n'
        '    <key>ProcessType</key>\n'
        '    <string>Interactive</string>\n'
        '</dict>\n'
        '</plist>\n'
    ).format(label=_xml_escape(LAUNCH_AGENT_LABEL),
             args=args_xml,
             log=_xml_escape(log_path))


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

    def install(self, log_path: str, agent_path: str, launchctl_fn) -> "tuple[bool, str]":
        """Set up the global-hotkey mechanism: compile binary, write the
        LaunchAgent plist, and load it.  Returns (ok: bool, detail: str).
        Non-fatal: absent swiftc returns (False, 'swiftc not found').

        *log_path*     – path to the hotkeyd log file.
        *agent_path*   – path where the LaunchAgent plist is written.
        *launchctl_fn* – callable(args) → int; abstracted so tests can patch it.
        """
        ok, detail = self.build()
        if not ok:
            return (ok, detail)
        plist_xml = _hotkeyd_plist(str(paths.HOTKEYD_BIN_PATH), log_path)
        os.makedirs(os.path.dirname(agent_path), exist_ok=True)
        with open(agent_path, "w", encoding="utf-8") as fh:
            fh.write(plist_xml)
        launchctl_fn(["unload", agent_path])
        load_rc = launchctl_fn(["load", agent_path])
        if load_rc != 0:
            return (True, "launchctl load returned {0}".format(load_rc))
        return (True, str(paths.HOTKEYD_BIN_PATH))

    def uninstall(self) -> None:
        try:
            os.remove(str(paths.HOTKEYD_BIN_PATH))
        except OSError:
            pass
