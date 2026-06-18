"""Shared localhost-TCP transport for the Sonari daemon <-> clients.

A lockfile (JSON: host/port/token/pid, mode 0o600) advertises the daemon's
ephemeral port + a 256-bit token. Loopback TCP has no filesystem ACL, so the
token is MANDATORY: a connection must send the token as its first line before
any message is processed."""
from __future__ import annotations

import json
import os
import socket
import sys

HOST = "127.0.0.1"


def make_token() -> str:
    import secrets
    return secrets.token_hex(32)  # 256-bit


def write_lockfile(path, host, port, token, pid) -> None:
    data = {"host": host, "port": int(port), "token": token, "pid": int(pid)}
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
    POSIX: fcntl.flock (content-independent). Windows: msvcrt.locking on a FIXED
    byte of a NON-truncated file — byte-range locks are system-wide, giving real
    cross-process exclusion; truncating under another holder's lock is undefined,
    and a moving file position would lock the wrong byte. The OS releases the
    lock on process death, so a crash never sticks.

    NOTE: cross-process exclusion on Windows MUST be confirmed on the box
    (M2-WINDOWS-ACCEPTANCE.md). If msvcrt.locking proves unreliable, switch to a
    named mutex (kernel32.CreateMutexW + GetLastError()==ERROR_ALREADY_EXISTS)."""
    fd = os.open(str(path), os.O_RDWR | os.O_CREAT, 0o600)
    fh = os.fdopen(fd, "r+")
    if sys.platform == "win32":
        import msvcrt
        fh.seek(0)
        try:
            msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)   # lock byte [0, 1)
        except OSError:
            fh.close()
            return None
    else:
        import fcntl
        try:
            fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            fh.close()
            return None
    try:
        fh.seek(0); fh.write(str(os.getpid())); fh.flush(); fh.truncate()
    except OSError:
        pass
    return fh
