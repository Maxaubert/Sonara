"""One TCP JSON-lines connection to the runtime."""
from __future__ import annotations

import itertools
import json
import socket
import threading
from typing import Optional

from .errors import E_CLOSED, SonaraError

MAX_LINE = 1 << 20


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

    def request(self, type_: str, fields: Optional[dict] = None) -> dict:
        """Send ``{type, **fields}``; the ``ok: true`` reply, or SonaraError."""
        with self._lock:
            if self.closed:
                raise SonaraError(E_CLOSED, "the connection is closed")
            msg = dict(fields or {})
            msg["type"] = type_
            msg["id"] = next(self._ids)
            try:
                self._sock.sendall(json.dumps(msg).encode("utf-8") + b"\n")
            except OSError as e:
                self._mark_closed()
                raise SonaraError(E_CLOSED, f"send failed: {e}") from None
            while True:
                try:
                    reply = self._read(self._timeout)
                except socket.timeout:
                    # A late reply would answer the next request: give up
                    # on this connection instead.
                    self._mark_closed()
                    raise SonaraError(E_CLOSED, f"no reply within {self._timeout} s") from None
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
                raise SonaraError(err.get("code", "E_BAD_REQUEST"), err.get("message", "request failed"))

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
