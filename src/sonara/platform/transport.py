"""Shared localhost-TCP transport for the Sonara daemon <-> clients.

A lockfile (JSON: host/port/token/pid, mode 0o600) advertises the daemon's
ephemeral port + a 256-bit token. Loopback TCP has no filesystem ACL, so the
token is MANDATORY: a connection must send the token as its first line before
any message is processed. OS-free: the daemon's single-instance guard lives in
platform/windows/singleton.py, behind sonara.platform.daemon_process()."""
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
