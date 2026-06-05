from __future__ import annotations

import socket
import time

from sonari.protocol import encode, decode
from sonari.paths import SOCKET_PATH, socket_connectable
from sonari.daemon import ensure_running


class DaemonNotRunning(OSError):
    """Raised when the Sonari daemon socket cannot be reached."""


def send(msg: dict, expect_reply: bool = False, timeout: float = 2.0):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        try:
            s.connect(str(SOCKET_PATH))
        except (ConnectionRefusedError, FileNotFoundError, OSError) as exc:
            raise DaemonNotRunning(
                "Sonari daemon is not running. Run: sonari install"
            ) from exc
        s.sendall(encode(msg))
        if not expect_reply:
            return None
        buf = b""
        while b"\n" not in buf:
            data = s.recv(4096)
            if not data:
                break
            buf += data
        if not buf:
            return None
        line = buf.split(b"\n", 1)[0]
        return decode(line)
    finally:
        try:
            s.close()
        except OSError:
            pass


def ensure_daemon(timeout: float = 3.0) -> None:
    if _connectable():
        return
    ensure_running()
    deadline = time.time() + timeout
    while time.time() < deadline:
        if _connectable():
            return
        time.sleep(0.05)


def _connectable() -> bool:
    return socket_connectable()
