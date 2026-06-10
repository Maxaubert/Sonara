"""macOS supervisor backend — launchd/launchctl install + python resolution."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
from typing import Optional

from sonari import paths
from sonari.platform.base import SupervisorBackend

LAUNCH_AGENT_LABEL = "com.sonari.speechd"
LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.sonari.speechd.plist")
_PYTHON_CANDIDATE_NAMES = (
    "python3", "python3.13", "python3.12", "python3.11", "python3.10", "python3.9",
)


def _launcher_path() -> str:
    return os.path.join(os.path.expanduser("~"), ".local", "bin", "sonari")


def _local_bin_dir() -> str:
    return os.path.join(os.path.expanduser("~"), ".local", "bin")


def _local_bin_on_path() -> bool:
    lb = _local_bin_dir()
    entries = os.environ.get("PATH", "").split(os.pathsep)
    return lb in entries


def _xml_escape(s: str) -> str:
    """Escape the three XML-significant characters for safe plist interpolation."""
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


class MacSupervisorBackend(SupervisorBackend):
    # --- python resolution (verbatim move of cli._resolve_python et al.) ---
    def _probe_python_version(self, candidate: str):
        """Return (major, minor) reported by *candidate*, or None if it cannot be run.

        Patched in tests. Runs the interpreter so we read its REAL version, not the
        one running cli.py.
        """
        try:
            out = subprocess.check_output(
                [candidate, "-c", "import sys; print('%d.%d' % sys.version_info[:2])"],
                stderr=subprocess.DEVNULL, text=True, timeout=5).strip()
            major, minor = out.split(".")
            return (int(major), int(minor))
        except Exception:  # noqa: BLE001 - any failure means "not a usable python"
            return None

    def resolve_python(self):
        """Return the absolute realpath of the best python3 >= 3.9, or None.

        Preference: /usr/bin/python3 when it qualifies (guaranteed present and stable
        across logins); otherwise the first qualifying candidate in PATH order.
        Candidates are deduped by realpath so a symlink farm is probed once.
        """
        candidates = ["/usr/bin/python3"]
        for name in _PYTHON_CANDIDATE_NAMES:
            found = shutil.which(name)
            if found:
                candidates.append(found)

        seen = set()
        qualifying = []  # list of (realpath, was_usr_bin)
        for cand in candidates:
            real = os.path.realpath(cand)
            if real in seen:
                continue
            seen.add(real)
            ver = self._probe_python_version(cand)
            if ver is not None and ver >= (3, 9):
                qualifying.append((real, cand == "/usr/bin/python3"))

        if not qualifying:
            return None
        for real, was_usr_bin in qualifying:
            if was_usr_bin:
                return real
        return qualifying[0][0]

    # --- launchd helpers (verbatim moves) ---
    def launchctl(self, args: list) -> int:
        """Run 'launchctl <args...>'. Returns the exit code."""
        try:
            return subprocess.call(["launchctl", *args])
        except FileNotFoundError:
            return 1

    def plist(self, label: str, program_args: list, log_path: str,
              env: Optional[dict] = None) -> str:
        """Return a full LaunchAgent plist XML for *label*.

        *program_args* is the ProgramArguments array (already absolute paths).
        *env*, when given, is emitted as an EnvironmentVariables <dict> (used to
        inject PYTHONPATH for the self-contained speech daemon). Every interpolated
        string is XML-escaped so a path containing &, <, or > cannot corrupt the
        plist. RunAtLoad + KeepAlive keep the agent alive in the Aqua (GUI) session;
        ProcessType Interactive so it participates in the foreground session that
        Carbon hotkeys require.
        """
        args_xml = "".join(
            "        <string>{0}</string>\n".format(_xml_escape(a))
            for a in program_args)
        env_xml = ""
        if env:
            pairs = "".join(
                "        <key>{0}</key>\n"
                "        <string>{1}</string>\n".format(
                    _xml_escape(k), _xml_escape(v))
                for k, v in env.items())
            env_xml = (
                '    <key>EnvironmentVariables</key>\n'
                '    <dict>\n'
                '{0}'
                '    </dict>\n'
            ).format(pairs)
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
            '{args_xml}'
            '    </array>\n'
            '{env_xml}'
            '    <key>RunAtLoad</key>\n'
            '    <true/>\n'
            '    <key>KeepAlive</key>\n'
            '    <true/>\n'
            '    <key>StandardErrorPath</key>\n'
            '    <string>{log_path}</string>\n'
            '    <key>StandardOutPath</key>\n'
            '    <string>{log_path}</string>\n'
            '    <key>ProcessType</key>\n'
            '    <string>Interactive</string>\n'
            '</dict>\n'
            '</plist>\n'
        ).format(
            label=_xml_escape(label),
            args_xml=args_xml,
            env_xml=env_xml,
            log_path=_xml_escape(log_path),
        )

    def launchagent_plist(self, python_executable: str, src_path: str,
                          log_path: str) -> str:
        """Return the LaunchAgent plist XML for the speech daemon."""
        return self.plist(
            LAUNCH_AGENT_LABEL,
            [python_executable, "-m", "sonari.daemon"],
            log_path,
            env={"PYTHONPATH": src_path},
        )

    # --- lifecycle ---
    def launch_spec(self):
        """Return (argv, spawn_kwargs) to lazily start the daemon process."""
        shim = os.path.join(paths.repo_root(), "bin", "sonari-daemon")
        return ([shim], {"start_new_session": True,
                         "stdin": subprocess.DEVNULL,
                         "stdout": subprocess.DEVNULL,
                         "stderr": subprocess.DEVNULL})

    def is_installed(self) -> bool:
        """True if ~/.local/bin/sonari exists (cheap stat)."""
        return os.path.exists(_launcher_path())

    def is_running(self) -> bool:
        """True if the daemon socket is accepting connections."""
        from sonari import paths as _p
        return _p.socket_connectable()

    def install(self, python, app_dir):  # filled in when Task 7 moves install
        pass

    def uninstall(self):
        pass

    def doctor_rows(self) -> list:
        """Return macOS-specific [(name, ok, detail), ...] diagnostic rows.

        Covers: say, afplay, enhanced voice, swiftc, hotkeyd binary,
        hotkeyd resolved keymap, speechd LaunchAgent loaded,
        hotkeyd LaunchAgent loaded, sonari launcher.
        """
        from sonari.platform.macos.hotkeys import (
            LAUNCH_AGENT_LABEL as HOTKEYD_LAUNCH_AGENT_LABEL,
        )
        rows = []

        # say
        say = shutil.which("say")
        rows.append(("say", say is not None,
                     say or "not found (macOS 'say' required)"))

        # afplay
        afplay = shutil.which("afplay")
        rows.append(("afplay", afplay is not None,
                     afplay or "not found (macOS 'afplay' required)"))

        # enhanced voice
        try:
            from sonari.platform.macos.tts import MacTtsBackend
            voice = MacTtsBackend().best_voice()
            rows.append(("enhanced voice", bool(voice),
                         voice or "none detected; will fall back to Samantha"))
        except Exception as exc:  # noqa: BLE001 - doctor must never raise
            rows.append(("enhanced voice", False, "error: {0}".format(exc)))

        # swiftc
        swiftc = shutil.which("swiftc")
        rows.append(("swiftc", swiftc is not None,
                     swiftc or "not found; install Command Line Tools: "
                               "xcode-select --install"))

        # hotkeyd binary
        hk_bin = str(paths.HOTKEYD_BIN_PATH)
        hk_exists = os.path.exists(hk_bin)
        rows.append(("hotkeyd binary", hk_exists,
                     hk_bin if hk_exists
                     else "missing: {0} (run 'sonari install')".format(hk_bin)))

        # hotkeyd resolved keymap
        try:
            with open(paths.HOTKEYD_RESOLVED_PATH, "r", encoding="utf-8") as fh:
                parsed = json.load(fh)
            ok = isinstance(parsed, list)
            rows.append(("hotkeyd resolved keymap", ok,
                         str(paths.HOTKEYD_RESOLVED_PATH) if ok
                         else "not a JSON list"))
        except Exception as exc:  # noqa: BLE001 - doctor must never raise
            rows.append(("hotkeyd resolved keymap", False,
                         "unreadable: {0}".format(exc)))

        # speechd LaunchAgent loaded
        speechd_loaded = self.launchctl(["list", LAUNCH_AGENT_LABEL]) == 0
        rows.append(("speechd LaunchAgent loaded", speechd_loaded,
                     LAUNCH_AGENT_LABEL if speechd_loaded
                     else "not loaded (run 'sonari install')"))

        # hotkeyd LaunchAgent loaded
        hotkeyd_loaded = self.launchctl(["list", HOTKEYD_LAUNCH_AGENT_LABEL]) == 0
        rows.append(("hotkeyd LaunchAgent loaded", hotkeyd_loaded,
                     HOTKEYD_LAUNCH_AGENT_LABEL if hotkeyd_loaded
                     else "not loaded (build CLT then 'sonari install')"))

        # sonari launcher + PATH
        launcher = _launcher_path()
        launcher_ok = os.path.exists(launcher)
        on_path = _local_bin_on_path()
        if launcher_ok and on_path:
            detail = launcher
        elif launcher_ok:
            detail = ("{0} present, but ~/.local/bin is NOT on PATH; add: "
                      'export PATH="$HOME/.local/bin:$PATH"').format(launcher)
        else:
            detail = "missing (run 'sonari install')"
        rows.append(("sonari launcher", launcher_ok and on_path, detail))

        return rows
