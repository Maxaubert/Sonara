"""Process-level startup hardening for the daemon on Windows: the native-crash
stack dump, the system VC++ runtime preload and the opt-out from power
throttling. main() runs all three before it builds any speech engine. Each is
best-effort and never raises."""
from __future__ import annotations

import os
import sys

_FAULT_FILE = None


def arm_faulthandler() -> None:
    """Dump every thread's Python stack to SONARA_DIR/faulthandler.log on a NATIVE
    crash (access violation / segfault in WinRT, ctypes, or winsound) -- the only
    way to see otherwise-silent C-level daemon deaths. Never raises."""
    global _FAULT_FILE
    try:
        import faulthandler
        # Import SONARA_DIR LIVE (not at module top) so the conftest monkeypatch /
        # any SONARA_DIR redirection takes effect; a top-level import would freeze
        # the value before tests patch it and leak into the real ~/.sonara.
        from sonara.paths import SONARA_DIR
        path = str(SONARA_DIR / "faulthandler.log")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        # Preserve a REAL crash dump before truncating (#65): every spawn
        # attempt (including instantly-exiting singleton losers) re-arms and
        # rewrote the file, so the silent-respawn flow destroyed the evidence
        # of the very crash it was healing seconds earlier. A file with more
        # than the one-line armed header is a dump: rotate it aside. A
        # header-only file is safe to truncate, so raced losers cannot rotate
        # the preserved dump away either.
        try:
            with open(path, encoding="utf-8") as fh:
                prior = fh.read(65536)
            if prior.count("\n") > 1:
                os.replace(path, str(SONARA_DIR / "faulthandler.prev.log"))
        except OSError:
            pass
        # mode 'w': only the latest run's crash matters; never grow unbounded.
        _FAULT_FILE = open(path, "w", encoding="utf-8")
        _FAULT_FILE.write("=== faulthandler armed: pid {0} ===\n".format(os.getpid()))
        _FAULT_FILE.flush()
        faulthandler.enable(file=_FAULT_FILE, all_threads=True)
    except Exception:  # noqa: BLE001 - diagnostics must never break startup
        pass


def preload_vc_runtime() -> None:
    """win32: preload the SYSTEM VC++ runtime before any speech engine import
    (#29). PyWinRT bundles an old MSVCP140.dll inside its package; whichever
    engine imports first binds its copy process-wide, and onnxruntime (Kokoro)
    crashes inside the old one ('DLL initialization routine failed') whenever a
    WinRT voice spoke first in this daemon's lifetime. The System32
    runtime is newer and serves BOTH engines, so loading it first makes engine
    import order irrelevant. Missing DLLs are tolerated: engines then fall back
    to their bundled copies exactly as before."""
    if sys.platform != "win32":
        return
    import ctypes
    root = os.environ.get("SystemRoot", r"C:\Windows")
    for dll in ("msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"):
        try:
            ctypes.WinDLL(os.path.join(root, "System32", dll))
        except OSError:
            pass


def harden_process(k32=None) -> None:
    """Keep the daemon responsive to global hotkeys even after long idle.

    Windows 11 puts idle, window-less background processes into EcoQoS / power
    throttling, and the Task Scheduler launches us at BelowNormal priority. A
    throttled hotkey-pump thread drops/delays the first WM_HOTKEY presses after a
    long idle (the "press 3-4 times before it registers" bug), and the timing skew
    occasionally double-fires a toggle. So at startup we (1) opt the process out of
    power throttling (ControlMask=EXECUTION_SPEED, StateMask=0 => "never throttle
    me") and (2) raise the priority class to Normal. Best-effort; never raises.
    *k32* is injectable for tests."""
    if sys.platform != "win32":
        return
    try:
        import ctypes
        from ctypes import wintypes
        if k32 is None:
            # Fresh WinDLL so the argtypes/restype we set here never mutate the
            # shared ctypes.windll.kernel32 used elsewhere. Proper HANDLE typing is
            # REQUIRED: GetCurrentProcess()'s pseudo-handle is -1, and without a
            # 64-bit HANDLE restype/argtype ctypes truncates it to a 32-bit value,
            # so both calls fail with ERROR_INVALID_HANDLE (6) and silently no-op.
            k32 = ctypes.WinDLL("kernel32", use_last_error=True)
            k32.GetCurrentProcess.restype = wintypes.HANDLE
            k32.SetProcessInformation.argtypes = [
                wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
            k32.SetProcessInformation.restype = wintypes.BOOL
            k32.SetPriorityClass.argtypes = [wintypes.HANDLE, wintypes.DWORD]
            k32.SetPriorityClass.restype = wintypes.BOOL

        class _PPTS(ctypes.Structure):
            _fields_ = [("Version", wintypes.DWORD),
                        ("ControlMask", wintypes.DWORD),
                        ("StateMask", wintypes.DWORD)]

        _PROCESS_POWER_THROTTLING = 4            # ProcessPowerThrottling info class
        _EXECUTION_SPEED = 0x1                   # PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        _NORMAL_PRIORITY_CLASS = 0x00000020
        h = k32.GetCurrentProcess()
        st = _PPTS(1, _EXECUTION_SPEED, 0)       # Version=1, control speed, state OFF
        k32.SetProcessInformation(h, _PROCESS_POWER_THROTTLING,
                                  ctypes.byref(st), ctypes.sizeof(st))
        k32.SetPriorityClass(h, _NORMAL_PRIORITY_CLASS)
    except Exception:  # noqa: BLE001 - hardening must never break startup
        pass
