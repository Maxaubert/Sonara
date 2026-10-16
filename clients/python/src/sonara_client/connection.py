"""One TCP JSON-lines connection to the runtime."""
from __future__ import annotations

import itertools
import json
import socket
import threading
from typing import Optional

from .errors import E_BAD_REQUEST, E_CLOSED, SonaraError

#: Longest line the runtime accepts (bytes, newline included); a longer one
#: is answered "line too long" and the runtime closes the connection.
MAX_LINE = 1 << 20

#: Seconds a request that asks the provider may take at least (#22): the
#: runtime answers ``engine_test`` only after one synthesis (up to a
#: profile's ``timeout_ms``, 60 s for Gemini or a local server), and a
#: ``refresh`` or draft ``profile`` waits on the provider's lists.
SLOW_TIMEOUT = 90.0


def is_slow(type_: str, fields: dict) -> bool:
    """True for the messages that wait on an external engine's provider."""
    if type_ == "engine_test":
        return True
    if type_ in ("voices", "engine_models"):
        return bool(fields.get("refresh")) or "profile" in fields
    return False


class Connection:
    """Requests and their replies (blocking, one at a time), plus events
    for a subscribed connection."""

    def __init__(self, port: int, timeout: float):
        try:
            self._sock = socket.create_connection(("127.0.0.1", port), timeout=timeout)
        except OSError as e:
            raise SonaraError(E_CLOSED, f"cannot connect to 127.0.0.1:{port}: {e}") from None
        self._sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self._timeout = timeout
        self._buf = b""
        self._ids = itertools.count(1)
        self._lock = threading.Lock()
        self._events: list = []
        self.closed = False

    def request(self, type_: str, fields: Optional[dict] = None, timeout: Optional[float] = None) -> dict:
        """Send ``{type, **fields}``; the ``ok: true`` reply, or SonaraError.
        ``timeout`` bounds this request (default: the connection's, and at
        least ``SLOW_TIMEOUT`` for a message that waits on a provider)."""
        msg = dict(fields or {})
        if timeout is None:
            timeout = max(self._timeout, SLOW_TIMEOUT) if is_slow(type_, msg) else self._timeout
        with self._lock:
            if self.closed:
                raise SonaraError(E_CLOSED, "the connection is closed")
            msg["type"] = type_
            msg["id"] = next(self._ids)
            line = json.dumps(msg).encode("utf-8") + b"\n"
            if len(line) > MAX_LINE:
                # Refused here, so the shared connection stays open (#54).
                raise SonaraError(E_BAD_REQUEST, f"line too long: {len(line)} bytes, at most {MAX_LINE}")
            try:
                self._sock.sendall(line)
            except OSError as e:
                self._mark_closed()
                raise SonaraError(E_CLOSED, f"send failed: {e}") from None
            while True:
                try:
                    reply = self._read(timeout)
                except socket.timeout:
                    # A late reply would answer the next request: give up
                    # on this connection instead.
                    self._mark_closed()
                    raise SonaraError(E_CLOSED, f"no reply within {timeout} s") from None
                if reply is None:
                    raise SonaraError(E_CLOSED, "the connection closed before the reply")
                if "event" in reply:
                    self._events.append(reply)
                    continue
                if "ok" not in reply:
                    continue
                if reply.get("ok") is True:
                    return reply
                err = reply.get("error") or {}
                raise SonaraError(err.get("code", "E_BAD_REQUEST"), err.get("message", "request failed"),
                                  err.get("reason"))

    def next_event(self, timeout: Optional[float]) -> Optional[dict]:
        """The next event; None at the end of the stream. ``socket.timeout``
        (``TimeoutError``) when none arrived within ``timeout`` seconds."""
        if self._events:
            return self._events.pop(0)
        while True:
            msg = self._read(timeout)
            if msg is None or "event" in msg:
                return msg

    def close(self) -> None:
        self._mark_closed()

    def _mark_closed(self) -> None:
        if not self.closed:
            self.closed = True
            try:
                self._sock.close()
            except OSError:
                pass

    def _read(self, timeout: Optional[float]) -> Optional[dict]:
        """The next JSON object line, or None once the stream ended."""
        while True:
            nl = self._buf.find(b"\n")
            if nl >= 0:
                line, self._buf = self._buf[:nl], self._buf[nl + 1:]
                line = line.strip()
                if not line:
                    continue
                try:
                    msg = json.loads(line)
                except ValueError:
                    continue
                if isinstance(msg, dict):
                    return msg
                continue
            if self.closed:
                return None
            self._sock.settimeout(timeout)
            try:
                chunk = self._sock.recv(65536)
            except socket.timeout:
                raise
            except OSError:
                chunk = b""
            if not chunk:
                self._mark_closed()
                return None
            self._buf += chunk
            if len(self._buf) > MAX_LINE * 2:
                self._mark_closed()
                return None
