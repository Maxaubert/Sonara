"""Shared localhost-TCP transport for the Sonara daemon <-> clients.

A lockfile (JSON: host/port/token/pid, mode 0o600) advertises the daemon's
ephemeral port + a 256-bit token. Loopback TCP has no filesystem ACL, so the
token is MANDATORY: a connection must send the token as its first line before
any message is processed."""
from __future__ import annotations

import json
import os
import socket
import time

HOST = "127.0.0.1"


def write_lockfile(path, host, port, token, pid, http_port=None) -> None:
    data = {"host": host, "port": int(port), "token": token, "pid": int(pid)}
    if http_port is not None:
        data["http_port"] = int(http_port)   # settings page (#34)
    tmp = str(path) + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(data, fh)
    os.chmod(tmp, 0o600)
    # E20: on Windows a hook process reading the old lockfile holds it open
    # without FILE_SHARE_DELETE, so the replace is denied for that moment.
    # Retry briefly instead of letting the daemon's startup die on it.
    for attempt in range(_SHARE_RETRIES):
        try:
            os.replace(tmp, str(path))
            return
        except PermissionError:
            if attempt == _SHARE_RETRIES - 1:
                raise
            time.sleep(_SHARE_DELAY)


# A sharing violation between the lockfile writer and its readers lasts
# milliseconds (one json read or write); this rides it out without stalling.
_SHARE_RETRIES = 20
_SHARE_DELAY = 0.025


def read_lockfile(path):
    for attempt in range(_SHARE_RETRIES):
        try:
            with open(str(path), "r", encoding="utf-8") as fh:
                return json.load(fh)
        except PermissionError:
            # Mid-replace on Windows (E20): the file is there, just briefly
            # unopenable. Retry rather than report "no daemon".
            if attempt == _SHARE_RETRIES - 1:
                return None
            time.sleep(_SHARE_DELAY)
        except (OSError, ValueError):
            return None
    return None


def connect(path, timeout=2.0):
    """Return a connected, authenticated socket, or raise OSError."""
    info = read_lockfile(path)
    if not info:
        raise OSError("daemon lockfile missing")
    try:
        host, port, token = info["host"], info["port"], info["token"]
    except (KeyError, TypeError) as exc:
        raise OSError("daemon lockfile is damaged: {0!r}".format(exc)) from exc
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        s.settimeout(timeout)
        s.connect((host, port))
        s.sendall((token + "\n").encode("utf-8"))   # token handshake first
    except BaseException:
        s.close()                  # E18: never leak the socket on a failed connect
        raise
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
_MUTEX_PREFIX = "Global\\Sonara-Daemon-Singleton-v2-"
_ERROR_ALREADY_EXISTS = 183
# The fixed name 0.6.6 and earlier held. It does not exclude the v2 name, so a
# daemon that survived an upgrade is probed by it for one release.
LEGACY_MUTEX_NAME = "Global\\Sonara-Daemon-Singleton-v1"
_SYNCHRONIZE = 0x00100000


def singleton_mutex_name(sonara_dir) -> str:
    """The mutex name for one user's Sonara (M11): keyed by that user's
    ~/.sonara, so another Windows user's daemon neither blocks this one (the
    old fixed Global\\ name was access-denied across users, and that read as
    "already owned") nor is blocked by it. Global\\ still spans this user's
    own sessions (console and RDP), which share one ~/.sonara."""
    import hashlib
    key = os.path.normcase(os.path.abspath(str(sonara_dir))).lower()
    return _MUTEX_PREFIX + hashlib.sha256(key.encode("utf-8")).hexdigest()[:24]


def default_mutex_name() -> str:
    from sonara import paths
    return singleton_mutex_name(paths.SONARA_DIR)


def acquire_singleton_mutex(name=None, kernel32=None, last_error=None):
    """Create/own the named single-instance mutex. Returns an opaque handle to
    hold for the process's lifetime, or None if another process already owns it.
    Raises OSError when the mutex cannot be created at all (M11): that used to
    read as "owned", and the daemon exited without a word.
    Non-Windows (no such API): returns a truthy sentinel so callers don't gate on
    it (the byte-lock remains the guard there)."""
    if os.name != "nt":
        return True
    probe_legacy = name is None
    if name is None:
        name = default_mutex_name()
    if kernel32 is None:
        import ctypes
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateMutexW.restype = ctypes.c_void_p
        kernel32.OpenMutexW.restype = ctypes.c_void_p
        kernel32.CloseHandle.argtypes = [ctypes.c_void_p]   # 64-bit handles
        last_error = ctypes.get_last_error
    handle = kernel32.CreateMutexW(None, True, name)   # bInitialOwner=True
    err = last_error()
    if not handle:
        raise OSError("CreateMutexW failed for {0}: error {1}".format(name, err))
    if err == _ERROR_ALREADY_EXISTS:
        kernel32.CloseHandle(handle)
        return None
    if probe_legacy and _legacy_mutex_held(kernel32):
        kernel32.CloseHandle(handle)
        return None
    return handle


def _legacy_mutex_held(kernel32) -> bool:
    """True when an older daemon (0.6.6 or earlier) still holds the v1 name.
    Another user's v1 daemon is access-denied, so it reads as absent, which
    is the per-user scoping M11 wants anyway."""
    legacy = kernel32.OpenMutexW(_SYNCHRONIZE, False, LEGACY_MUTEX_NAME)
    if not legacy:
        return False
    kernel32.CloseHandle(legacy)
    return True


def release_singleton_mutex(handle) -> None:
    """Release a handle from acquire_singleton_mutex (the OS also frees it on
    process death, so this is only needed for explicit early teardown / tests)."""
    if os.name != "nt" or not handle or handle is True:
        return
    import ctypes
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CloseHandle(ctypes.c_void_p(handle))
