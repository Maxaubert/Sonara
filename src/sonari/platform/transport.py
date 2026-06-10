"""Shared localhost-TCP transport for the Sonari daemon <-> clients.

A lockfile (JSON: host/port/token/pid, mode 0o600) advertises the daemon's
ephemeral port + a 256-bit token. Loopback TCP has no filesystem ACL, so the
token is MANDATORY: a connection must send the token as its first line before
any message is processed."""
from __future__ import annotations

import json
import os
import socket

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
    """Acquire an exclusive, OS-level single-instance lock at *path*.

    Returns the held file object on success — the CALLER MUST keep a reference
    for the whole process lifetime, since the flock is released when the file
    object is garbage-collected/closed (or, automatically, when the process
    dies — so a crashed daemon never leaves a stuck lock). Returns None if
    another live process already holds it.

    This restores the single-instance guarantee that the fixed-path AF_UNIX
    bind() gave us for free; with an ephemeral TCP port, bind() never collides,
    so this flock is the authoritative guard against duplicate daemons.
    (POSIX/fcntl for now; the Windows backend supplies its own in M2.)"""
    import fcntl
    fh = open(str(path), "w")
    try:
        fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        fh.close()
        return None
    try:
        fh.write(str(os.getpid()))
        fh.flush()
    except OSError:
        pass
    return fh
