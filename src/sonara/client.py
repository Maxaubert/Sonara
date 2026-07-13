from __future__ import annotations

import time

from sonara.protocol import encode, decode
from sonara.paths import LOCK_PATH, socket_connectable
from sonara.platform import transport
from sonara.daemon import ensure_running


class DaemonNotRunning(OSError):
    """Raised when the Sonara daemon socket cannot be reached."""


def send(msg: dict, expect_reply: bool = False, timeout: float = 2.0):
    try:
        s = transport.connect(LOCK_PATH, timeout=timeout)
    except OSError as exc:
        raise DaemonNotRunning(
            "Sonara daemon is not running. Run: sonara start"
        ) from exc
    try:
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
