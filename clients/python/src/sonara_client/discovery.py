"""Find, check and start the runtime (protocol v1, Discovery)."""
from __future__ import annotations

import json
import os
import subprocess
import time
from typing import Optional, Sequence

from .errors import E_NOT_RUNNING, E_START_FAILED, SonaraError

# Exit code of a second sonarad for the same user and home.
EXIT_ALREADY_RUNNING = 3
POLL = 0.05
CREATE_NO_WINDOW = 0x08000000
CREATE_NEW_PROCESS_GROUP = 0x00000200

# Started runtimes outlive this process (they are shared and exit on their
# own when idle). Holding the Popen objects keeps Python from warning that a
# child is still running when they are garbage collected.
_started: list = []


def resolve_home(home: Optional[str] = None, env=None) -> str:
    """``home``, else ``SONARA_HOME``, else ``%LOCALAPPDATA%\\Sonara``."""
    env = os.environ if env is None else env
    if home:
        return str(home)
    if env.get("SONARA_HOME"):
        return env["SONARA_HOME"]
    if env.get("LOCALAPPDATA"):
        return os.path.join(env["LOCALAPPDATA"], "Sonara")
    raise SonaraError(E_NOT_RUNNING, "no home folder: pass home, or set SONARA_HOME or LOCALAPPDATA")


def read_runtime(home: str) -> Optional[dict]:
    """``runtime.json`` of ``home``, or None when missing or unreadable."""
    try:
        with open(os.path.join(home, "runtime.json"), encoding="utf-8") as f:
            info = json.load(f)
    except (OSError, ValueError):
        return None
    if not isinstance(info, dict):
        return None
    if not isinstance(info.get("pid"), int) or not isinstance(info.get("port"), int):
        return None
    if not isinstance(info.get("token"), str):
        return None
    return info


def pid_alive(pid: int) -> bool:
    """True while process ``pid`` exists."""
    if not isinstance(pid, int) or pid <= 0:
        return False
    if os.name == "nt":
        return _pid_alive_windows(pid)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def _pid_alive_windows(pid: int) -> bool:
    # Not os.kill(pid, 0): on Windows that terminates the process.
    import ctypes
    from ctypes import wintypes

    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.OpenProcess.restype = wintypes.HANDLE
    k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    k32.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    k32.CloseHandle.argtypes = [wintypes.HANDLE]
    process_query_limited_information = 0x1000
    still_active = 259
    handle = k32.OpenProcess(process_query_limited_information, False, pid)
    if not handle:
        # Access denied: it exists but belongs to someone else.
        return ctypes.get_last_error() == 5
    try:
        code = wintypes.DWORD()
        if not k32.GetExitCodeProcess(handle, ctypes.byref(code)):
            return True
        return code.value == still_active
    finally:
        k32.CloseHandle(handle)


def live_runtime(home: str) -> Optional[dict]:
    """``runtime.json`` of ``home`` when its pid is alive."""
    info = read_runtime(home)
    return info if info and pid_alive(info["pid"]) else None


def start_runtime(runtime_path: str, home: str, args: Sequence[str], timeout: float) -> dict:
    """Start ``runtime_path --home <home> [args]`` without a console window
    and wait up to ``timeout`` seconds for its ``runtime.json``. When another
    client started one at the same moment (the new process exits with code
    3), the other's ``runtime.json`` is used."""
    kwargs: dict = {
        "stdin": subprocess.DEVNULL,
        "stdout": subprocess.DEVNULL,
        "stderr": subprocess.DEVNULL,
        "close_fds": True,
    }
    if os.name == "nt":
        # Its own process group, so a Ctrl+C in this console does not end
        # a runtime other apps share.
        kwargs["creationflags"] = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
    else:
        kwargs["start_new_session"] = True
    try:
        proc = subprocess.Popen([str(runtime_path), "--home", str(home), *args], **kwargs)
    except OSError as e:
        raise SonaraError(E_START_FAILED, f"cannot start {runtime_path}: {e}") from None
    _started.append(proc)

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        info = read_runtime(home)
        if info and info["pid"] == proc.pid:
            return info
        code = proc.poll()
        if code is not None:
            if code != EXIT_ALREADY_RUNNING:
                raise SonaraError(E_START_FAILED, f"{runtime_path} exited with code {code}")
            other = live_runtime(home)
            if other:
                return other
        time.sleep(POLL)
    raise SonaraError(E_START_FAILED, f"{runtime_path} wrote no runtime.json within {timeout} s")


def wait_exit(pid: int, timeout: float) -> bool:
    """Wait until process ``pid`` has ended; False if it still runs."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not pid_alive(pid):
            return True
        time.sleep(POLL)
    return not pid_alive(pid)
