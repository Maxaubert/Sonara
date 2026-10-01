"""Windows supervisor backend -- zero-admin Task Scheduler autostart, Python
resolution (py-launcher + Store-stub avoidance), the sonara.cmd launcher, and
the WinSupervisorBackend ABC implementation. The Claude Code hooks it installs
are written by sonara.install.claude_hooks.

WINDOWS-only. Every Windows-only stdlib import (winreg, ctypes) is lazy (inside
a method/function) so this module imports cleanly off Windows for the mock
test suite. "Importable + mock-green" here does NOT mean Windows-verified -- the
real gate is docs/history/M2-WINDOWS-ACCEPTANCE.md.

Bodies copied verbatim from docs/history/m2-windows-api-reference.md
(§Windows SupervisorBackend), adapting only: the file/import location to our
layout (src/sonara/platform/windows/...), subclassing the real ABC from
sonara.platform.base, and keeping Windows-only imports lazy.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from typing import Optional

from sonara import paths
from sonara.platform.base import SupervisorBackend

TASK_NAME = "Sonara.Speechd"

# Daemon process-creation flags live with the single launch_spec in
# supervisor_loop (#123). This module's copy went dead when launch_spec was
# de-duplicated; a second definition here is exactly how the two spawn paths
# drifted in the first place.


# ---------------------------------------------------------------------------
# Zero-admin Task Scheduler autostart via hand-authored XML
# ---------------------------------------------------------------------------

# UTF-16 LE with BOM is required by schtasks /xml on older Windows builds.
# Python's encoding='utf-16' produces exactly that.
TASK_XML_TEMPLATE = '''<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2"
  xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>{user_id}</Author>
    <Description>Sonara speech daemon supervisor (autostart on logon)</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user_id}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <Hidden>true</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <!-- 5 = NORMAL priority class. The default 7 (BelowNormal) makes Windows
         power-throttle the idle daemon, delaying global-hotkey response. -->
    <Priority>5</Priority>
    <RestartOnFailure>
      <Interval>PT5M</Interval>
      <Count>5</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{pythonw}</Command>
      <Arguments>"{supervisor_py}"</Arguments>
      <WorkingDirectory>{work_dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
'''


def _current_user_id() -> str:
    """Return DOMAIN\\user or COMPUTERNAME\\user for LogonTrigger/UserId."""
    import ctypes
    buf = ctypes.create_unicode_buffer(256)
    size = ctypes.c_ulong(256)
    ctypes.windll.secur32.GetUserNameExW(2, buf, ctypes.byref(size))  # 2 = NameSamCompatible
    return buf.value


def task_install(pythonw: str, supervisor_py: str) -> int:
    """Register the Task Scheduler task. Returns schtasks exit code (0 = success)."""
    from xml.sax.saxutils import escape
    user_id = _current_user_id()
    paths.ensure_sonara_dir()
    xml_content = TASK_XML_TEMPLATE.format(
        # Escaped: an '&' or '<' in a user or folder name broke the XML (L-xml).
        user_id=escape(user_id),
        pythonw=escape(pythonw),
        supervisor_py=escape(supervisor_py),
        # ~/.sonara, not the script's folder: that sat inside app/sonara, the
        # tree install renames and uninstall deletes, and a lingering
        # supervisor pinned it (E14). The script path is absolute, and
        # supervisor_loop derives its imports from __file__.
        work_dir=escape(str(paths.SONARA_DIR)),
    )
    # Write UTF-16 LE with BOM -- required by schtasks /xml
    with tempfile.NamedTemporaryFile(
            mode='w', suffix='.xml', encoding='utf-16',
            delete=False) as fh:
        fh.write(xml_content)
        tmp = fh.name
    try:
        return subprocess.call(
            ["schtasks", "/create", "/tn", TASK_NAME, "/xml", tmp, "/f"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
    finally:
        os.unlink(tmp)


def task_uninstall() -> int:
    """Delete the task. /f suppresses confirmation prompt."""
    return subprocess.call(
        ["schtasks", "/delete", "/tn", TASK_NAME, "/f"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )


# KEY GOTCHA: RestartOnFailure is NOT expressible via schtasks CLI flags -- XML only.
# The Task Scheduler's RestartOnFailure only restarts the *supervisor* process if
# it crashes (unlikely). The supervisor_loop is the real daemon restarter.


# ---------------------------------------------------------------------------
# Windows Python resolution -- py -3 launcher, PATH probe, Store-stub detection
# ---------------------------------------------------------------------------

# Every probe below can run under a consoleless parent (a pythonw hook, the
# settings-page respawner, the daemon's lazy start). A console child of such a
# parent pops a visible console window unless spawned windowless (E9, #125).
_NO_WINDOW = 0x08000000   # CREATE_NO_WINDOW (hex: imports on POSIX)

def _is_store_stub(path: str) -> bool:
    """Return True if *path* is the Windows Store Python stub.

    Fast path: WindowsApps in the normalised path.
    Slow path: run it and check for exit code 9009 (store stub sentinel) or
    empty stdout (the stub prints nothing and exits non-zero).
    """
    if "WindowsApps" in os.path.normcase(path):
        return True
    try:
        result = subprocess.run(
            [path, "-c", "import sys; print(sys.executable)"],
            capture_output=True, text=True, timeout=5, creationflags=_NO_WINDOW,
        )
        return result.returncode == 9009 or not result.stdout.strip()
    except Exception:
        return True   # treat anything broken as a stub


def _find_pythonw(python_real: str) -> "str | None":
    """Return the pythonw.exe sibling of *python_real*, or None."""
    d = os.path.dirname(python_real)
    for candidate in (
        os.path.join(d, "pythonw.exe"),
        os.path.join(d, "Scripts", "pythonw.exe"),   # venv layout
    ):
        if os.path.isfile(candidate):
            return candidate
    return None


def _probe_python_version(candidate: str):
    """Return (major, minor) or None. The one version probe: the backend
    method delegates here (L-interp-dup)."""
    try:
        out = subprocess.check_output(
            [candidate, "-c",
             "import sys; print('%d.%d' % sys.version_info[:2])"],
            stderr=subprocess.DEVNULL, text=True, timeout=5,
            creationflags=_NO_WINDOW,
        ).strip()
        major, minor = out.split(".")
        return (int(major), int(minor))
    except Exception:
        return None


def _probe_version_via_launcher(py_exe: str) -> "str | None":
    """Use `py -3 -c 'print(sys.executable)'` to resolve the real interpreter."""
    try:
        real = subprocess.check_output(
            [py_exe, "-3", "-c", "import sys; print(sys.executable)"],
            stderr=subprocess.DEVNULL, text=True, timeout=5,
            creationflags=_NO_WINDOW,
        ).strip()
        return real if real else None
    except Exception:
        return None


def daemon_pythonw() -> "str | None":
    """The pythonw.exe the daemon should run on: the neural venv's when neural is
    enabled AND it probes >=3.10, else the system pythonw. Windows analog of
    cli._daemon_python (both pick the venv through kp.usable_venv_python),
    yielding the windowless interpreter for the background daemon."""
    from sonara import kokoro_provision as kp
    venv_py = kp.usable_venv_python(_probe_python_version)
    if venv_py:
        return _find_pythonw(venv_py) or venv_py
    return resolve_python_windows()


def resolve_python_windows() -> "str | None":
    """Return pythonw.exe path for the best Python 3 >= 3.9, or None.

    Resolution order:
      1. py -3 launcher (works even when python.exe is not on PATH)
      2. 'python' on PATH (skip Microsoft Store stubs)
      3. 'python3' on PATH (skip Microsoft Store stubs)
    Deduped by realpath; prefers the py-launcher result.
    """
    seen_real = set()
    candidates = []   # list of (real_python_path, source_label)

    # 1. Windows Python Launcher
    py = shutil.which("py")
    if py:
        real = _probe_version_via_launcher(py)
        if real and not _is_store_stub(real):
            candidates.append((real, "py-launcher"))

    # 2 & 3. PATH-based names
    for name in ("python", "python3"):
        found = shutil.which(name)
        if found and not _is_store_stub(found):
            try:
                real = subprocess.check_output(
                    [found, "-c", "import sys; print(sys.executable)"],
                    stderr=subprocess.DEVNULL, text=True, timeout=5,
                    creationflags=_NO_WINDOW,
                ).strip()
            except Exception:
                continue
            if real:
                candidates.append((real, name))

    for real, _src in candidates:
        norm = os.path.normcase(os.path.realpath(real))
        if norm in seen_real:
            continue
        seen_real.add(norm)
        ver = _probe_python_version(real)
        if ver and ver >= (3, 9):
            pw = _find_pythonw(real)
            if pw:
                return pw

    # No usable system Python -> fall back to the interpreter the bootstrap
    # provisioned + recorded (the zero-Python install path).
    return paths.recorded_pythonw()


# ---------------------------------------------------------------------------
# Stray daemon sweep (#65)
# ---------------------------------------------------------------------------

def kill_stray_daemons(runner=None) -> int:
    """Terminate any `-m sonara.daemon` processes still alive after the socket
    owner died (#65). SHUTDOWN only reaches the lockfile/socket owner; a
    split-brain survivor (an older daemon that lost the socket race but still
    holds the global hotkeys) outlives every `sonara shutdown` and keeps
    swallowing hotkey presses - mute appears broken. Best-effort: returns the
    number of processes killed, 0 on any failure."""
    script = (
        "Get-CimInstance Win32_Process -Filter \"Name like 'python%'\" | "
        "Where-Object { $_.CommandLine -match 'sonara[.]daemon' } | "
        "ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue; "
        "$_.ProcessId }")
    run = runner or (lambda argv: subprocess.run(
        argv, capture_output=True, timeout=15,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)))
    try:
        proc = run(["powershell", "-NoProfile", "-Command", script])
        killed = [ln for ln in (proc.stdout or b"").decode("utf-8", "replace").split()
                  if ln.strip().isdigit()]
        if killed:
            print("stopped {0} stray daemon process(es): {1}".format(
                len(killed), ", ".join(killed)))
        return len(killed)
    except Exception:  # noqa: BLE001 - a failed sweep must never fail a stop
        return 0


# ---------------------------------------------------------------------------
# Windows launcher (the ~/.local/bin/sonara analogue: a sonara.cmd shim)
# ---------------------------------------------------------------------------

def _local_bin_dir() -> str:
    return os.path.join(os.path.expanduser("~"), ".local", "bin")


def _console_python(pythonw: str) -> str:
    """python.exe sibling of pythonw.exe (console interpreter, for the CLI launcher)."""
    cand = pythonw.replace("pythonw.exe", "python.exe")
    return cand if os.path.isfile(cand) else pythonw


# ---------------------------------------------------------------------------
# WinSupervisorBackend -- the SupervisorBackend ABC implementation
# ---------------------------------------------------------------------------

class WinSupervisorBackend(SupervisorBackend):

    # --- monkeypatchable thin wrappers ---

    def _schtasks(self, args: list) -> int:
        """Run 'schtasks <args>'. Monkeypatched in tests.

        CREATE_NO_WINDOW is required: is_installed() runs inside the daemon
        (pythonw, no console) at every SESSION_START, and a console child of a
        consoleless parent otherwise pops a visible console window (#125).
        """
        return subprocess.call(
            ["schtasks"] + args,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=0x08000000,  # CREATE_NO_WINDOW (hex: imports on POSIX)
        )

    def _probe_python_version(self, candidate: str):
        """Return (major, minor) or None. Monkeypatched in tests."""
        return _probe_python_version(candidate)

    # --- SupervisorBackend ABC ---

    def is_installed(self) -> bool:
        """Return True if the Task Scheduler task exists."""
        return self._schtasks(["/query", "/tn", TASK_NAME]) == 0

    def is_running(self) -> bool:
        """Return True if the daemon socket is accepting connections."""
        from sonara import paths
        return paths.socket_connectable()

    def resolve_python(self) -> Optional[str]:
        """Return pythonw.exe for the best Python >= 3.9, or None."""
        return resolve_python_windows()

    def launch_spec(self) -> tuple:
        """Return (argv, spawn_kwargs) for lazy daemon start.

        Delegates to the supervisor_loop implementation so the hook lazy start
        and the Task Scheduler loop spawn the daemon IDENTICALLY (#123). This
        used to be a hand-maintained second copy whose comment promised "parity
        with WinSupervisorBackend.launch_spec"; the two had already drifted on
        the one thing that matters most -- which copy of Sonara the daemon runs.

        The copy here derived PYTHONPATH from `repo_root() + "/src"`, valid only
        in a checkout. Started from the DEPLOYED runtime it prepended the
        nonexistent ~/.sonara/src and imported only because ~/.sonara/app
        happened to be inherited on PYTHONPATH. supervisor_loop derives it from
        __file__, which is right in both layouts.
        """
        from .supervisor_loop import launch_spec as _shared_launch_spec
        return _shared_launch_spec(daemon_pythonw() or "pythonw.exe")

    def doctor_rows(self) -> list:
        """Return Windows-specific [(name, ok, detail), ...] rows.

        Never raises -- wrap every external call in try/except so 'sonara doctor'
        always renders.
        """
        rows = []

        # schtasks availability
        schtasks = shutil.which("schtasks")
        rows.append(("schtasks", schtasks is not None,
                     schtasks or "not found (unexpected on Windows)"))

        # Task registered
        task_ok = self.is_installed()
        rows.append(("Task Scheduler task", task_ok,
                     TASK_NAME if task_ok
                     else "not registered (run 'sonara install')"))

        # pythonw.exe
        pw = self.resolve_python()
        rows.append(("pythonw.exe", pw is not None,
                     pw or "no Python >= 3.9 found; install from python.org"))

        # The Windows (OneCore) voice, checked by synthesizing for real: the
        # old registry read reported 'none' where WinRT lists voices, and a
        # listed voice can still lack its data files (D7).
        try:
            from sonara.platform.windows import tts as _tts
            ok, detail = _tts.probe_windows_voice()
            rows.append(("Windows voice", ok, detail))
        except Exception as exc:  # noqa: BLE001 - doctor must always render
            rows.append(("Windows voice", False, "error: {0}".format(exc)))

        # PyWinRT (the OneCore TTS engine). Absent -> total no-speech, so a
        # doctor green everywhere else would be dangerously misleading. (#7)
        try:
            from sonara.platform.windows.tts import _winrt_available
            ok = _winrt_available()
            rows.append(("TTS runtime", ok,
                         "PyWinRT ready" if ok else
                         "PyWinRT (winrt) not installed -> no speech. pip install "
                         "winrt-runtime winrt-Windows.Media.SpeechSynthesis "
                         "winrt-Windows.Storage.Streams"))
        except Exception as exc:  # noqa: BLE001 - doctor must always render
            rows.append(("TTS runtime", False, "error: {0}".format(exc)))

        # Daemon running
        running = self.is_running()
        rows.append(("daemon running", running,
                     "accepting connections" if running
                     else "not running (run 'sonara start')"))

        return rows

    def install(self, python: str, app_dir: str,
                plugin_root: "str | None" = None) -> None:
        pythonw = _find_pythonw(python) or python  # background daemon/hooks: no console window
        # 1. Claude Code hooks FIRST (exec-form in ~/.claude/settings.json,
        #    unless the enabled plugin supplies them). This is the step that can
        #    fail on a malformed user settings.json (it raises ValueError);
        #    doing it before the Task Scheduler registration means a failure
        #    leaves no orphaned autostart task behind (partial-install avoidance).
        from sonara.install import claude_hooks
        claude_hooks.install_hooks(pythonw, plugin_root)
        # 2. Task Scheduler autostart (pythonw runs the supervisor loop).
        supervisor_py = os.path.join(app_dir, "sonara", "platform",
                                     "windows", "supervisor_loop.py")
        # Best-effort stop the running task before overwriting its definition, so a
        # stale daemon on the OLD interpreter doesn't linger. /end is async and
        # /create does not auto-start, so
        # the new interpreter activates on the NEXT daemon start (next logon, or the
        # lazy-start path which now resolves the venv pythonw via daemon_pythonw()).
        self.end_task()
        rc = task_install(pythonw, supervisor_py)
        if rc == 0:
            print("Registered Task Scheduler task: {0}".format(TASK_NAME))
        else:
            print("warning: schtasks /create returned {0}; autostart may not be "
                  "registered.".format(rc))
        # 3. sonara.cmd launcher on ~/.local/bin.
        launcher = self._place_launcher(python, app_dir)
        print("Placed launcher: {0}".format(launcher))

    def _place_launcher(self, python: str, app_dir: str) -> str:
        path = os.path.join(_local_bin_dir(), "sonara.cmd")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        body = (
            "@echo off\r\n"
            'set "PYTHONPATH={app}"\r\n'
            '"{py}" -m sonara.cli %*\r\n'
        ).format(app=app_dir, py=_console_python(python))
        with open(path, "w", encoding="utf-8", newline="") as fh:
            fh.write(body)
        return path

    def end_task(self) -> None:
        """Best-effort end of the RUNNING scheduled task (the task-launched
        supervisor tree). Lazy-started daemons are stopped via the SHUTDOWN
        protocol message instead; install.service.stop_sonara() composes both
        (#23)."""
        self._schtasks(["/end", "/tn", TASK_NAME])

    def kill_stray_daemons(self) -> int:
        return kill_stray_daemons()

    def uninstall(self) -> None:
        rc = task_uninstall()
        print("Removed Task Scheduler task: {0}".format(TASK_NAME) if rc == 0
              else "No Task Scheduler task to remove.")
        from sonara.install import claude_hooks
        claude_hooks.uninstall_hooks()
        launcher = os.path.join(_local_bin_dir(), "sonara.cmd")
        if os.path.exists(launcher):
            try:
                os.remove(launcher)
                print("Removed launcher: {0}".format(launcher))
            except OSError:
                pass

    def post_install_notes(self) -> None:
        """Print the Windows post-install next steps."""
        print("")
        print("Sonara is installed. Run 'sonara doctor' to confirm everything is green.")
        # #19: hotkeys now ship and start with the daemon (no longer "M3-pending").
        print("  - Global hotkeys start automatically with the daemon; "
              "run 'sonara keymap' to see the bindings.")
        # The plugin's command files were renamed to NTFS-safe names (status.md,
        # voice.md, ...), so the /sonara:* slash commands now work on Windows too.
        print("  - Enable the 'sonara' plugin for its /sonara:* slash commands "
              "(optional; speech and hotkeys work without it).")

    def hooks_doctor_row(self) -> tuple:
        """The Claude Code hooks row (sonara.install.claude_hooks)."""
        from sonara.install import claude_hooks
        return claude_hooks.doctor_row()
