"""Shared localhost-TCP transport for the Sonara daemon <-> clients.

A lockfile (JSON: host/port/token/pid, mode 0o600) advertises the daemon's
ephemeral port + a 256-bit token. Loopback TCP has no filesystem ACL, so the
token is MANDATORY: a connection must send the token as its first line before
any message is processed."""
from __future__ import annotations

import json
import os
import socket

HOST = "127.0.0.1"


def write_lockfile(path, host, port, token, pid, http_port=None) -> None:
    data = {"host": host, "port": int(port), "token": token, "pid": int(pid)}
    if http_port is not None:
        data["http_port"] = int(http_port)   # settings page (#34)
    tmp = str(path) + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.chmod(tmp, 0o600)
    os.replace(tmp, str(path))


def read_lockfile(path):
    try:
        with open(str(path), "r", encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def connect(path, timeout=2.0):
    """Return a connected, authenticated socket, or raise OSError."""
    info = read_lockfile(path)
    if not info:
        raise OSError("daemon lockfile missing")
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect((info["host"], info["port"]))
    s.sendall((info["token"] + "\n").encode("utf-8"))   # token handshake first
    return s


def connectable(path) -> bool:
    try:
        s = connect(path, timeout=1.0)
    except OSError:
        return False
    try:
        s.close()
    except OSError:
        pass
    return True


def acquire_singleton(path):
    """Acquire an exclusive single-instance lock; return the held file object
    (keep a process-lifetime reference) or None if another process holds it.
    Windows: msvcrt.locking on a FIXED byte of a NON-truncated file -- byte-range
    locks are system-wide, giving real cross-process exclusion; truncating under
    another holder's lock is undefined, and a moving file position would lock the
    wrong byte. The OS releases the lock on process death, so a crash never sticks.

    NOTE: cross-process exclusion on Windows MUST be confirmed on the box
    (M2-WINDOWS-ACCEPTANCE.md). If msvcrt.locking proves unreliable, switch to a
    named mutex (kernel32.CreateMutexW + GetLastError()==ERROR_ALREADY_EXISTS)."""
    fd = os.open(str(path), os.O_RDWR | os.O_CREAT, 0o600)
    fh = os.fdopen(fd, "r+")
    import msvcrt
    fh.seek(0)
    try:
        msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)   # lock byte [0, 1)
    except OSError:
        fh.close()
        return None
    try:
        fh.seek(0); fh.write(str(os.getpid())); fh.flush(); fh.truncate()
    except OSError:
        pass
    return fh


# Windows named-mutex single-instance guard. The byte-lock above is fragile: it
# is tied to the lock FILE's identity, so a deleted/recreated lock file (or two
# daemons racing to create it) yields locks on different inodes that no longer
# exclude -> a daemon explosion (observed live). A named kernel mutex is keyed by
# NAME, shared by every process regardless of any file, and the kernel releases it
# on process death. This is the AUTHORITATIVE single-instance guard on Windows.
_MUTEX_NAME = "Global\\Sonara-Daemon-Singleton-v1"
_ERROR_ALREADY_EXISTS = 183


def acquire_singleton_mutex(name: str = _MUTEX_NAME):
    """Create/own the named single-instance mutex. Returns an opaque handle to
    hold for the process's lifetime, or None if another process already owns it.
    Non-Windows (no such API): returns a truthy sentinel so callers don't gate on
    it (the byte-lock remains the guard there)."""
    if os.name != "nt":
        return True
    import ctypes
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateMutexW.restype = ctypes.c_void_p
    handle = kernel32.CreateMutexW(None, True, name)   # bInitialOwner=True
    if not handle:
        return None
    if ctypes.get_last_error() == _ERROR_ALREADY_EXISTS:
        kernel32.CloseHandle(ctypes.c_void_p(handle))
        return None
    return handle


def release_singleton_mutex(handle) -> None:
    """Release a handle from acquire_singleton_mutex (the OS also frees it on
    process death, so this is only needed for explicit early teardown / tests)."""
    if os.name != "nt" or not handle or handle is True:
        return
    import ctypes
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CloseHandle(ctypes.c_void_p(handle))
