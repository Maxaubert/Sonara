from __future__ import annotations

import os
import socket
import time

from sonara import paths
from sonara.protocol import encode, decode
from sonara.paths import LOCK_PATH, socket_connectable
from sonara.platform import transport
from sonara.lifecycle import ensure_running


class DaemonNotRunning(OSError):
    """Raised when the Sonara daemon socket cannot be reached."""


class DaemonUnresponsive(OSError):
    """Raised when the daemon took the connection but did not answer in time,
    for example stuck under its lock (E18)."""


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
    except (socket.timeout, TimeoutError) as exc:
        raise DaemonUnresponsive("Sonara daemon is not responding") from exc
    finally:
        try:
            s.close()
        except OSError:
            pass


def send_many(msgs, timeout: float = 2.0) -> None:
    """Send every message of ONE hook event over ONE connection, in order.

    The daemon serves each connection on its own thread, so one connection
    per message let SET_FOREGROUND/FLUSH (or EARCON/CHOICE) of a single event
    be applied in either order (architecture review 0.5). On one connection
    the daemon applies them sequentially, as sent. No reply is read.
    Raises DaemonNotRunning when the daemon cannot be reached (nothing sent)."""
    payload = b"".join(encode(m) for m in msgs)
    if not payload:
        return
    try:
        s = transport.connect(LOCK_PATH, timeout=timeout)
    except OSError as exc:
        raise DaemonNotRunning(
            "Sonara daemon is not running. Run: sonara start"
        ) from exc
    try:
        s.sendall(payload)
    finally:
        try:
            s.close()
        except OSError:
            pass


def ensure_daemon(timeout: float = 3.0) -> None:
    if _connectable():
        return
    if os.path.exists(str(paths.STOPPED_SENTINEL_PATH)):
        # Shut down (or uninstalled): nothing will start, so do not wait for
        # it. Spinning the full timeout here delayed every hook event, and so
        # every tool call, by about 3 s after `sonara shutdown` (E7).
        return
    ensure_running()
    deadline = time.time() + timeout
    while time.time() < deadline:
        if _connectable():
            return
        time.sleep(0.05)


def _connectable() -> bool:
    return socket_connectable()
