"""The daemon's single-instance guard on Windows: a named kernel mutex (the
authoritative guard) plus a byte-lock on the lock file (a pid record, and the
fallback when the mutex cannot be created, M11). The daemon reaches it through
sonara.platform.daemon_process(), never directly, and it stays light: daemon
main() takes it before the VC runtime preload (#29)."""
from __future__ import annotations

import os


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
    """
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
    if not handle:
        return
    import ctypes
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CloseHandle(ctypes.c_void_p(handle))
