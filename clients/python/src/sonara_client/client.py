"""The connected client and its event subscriptions."""
from __future__ import annotations

import socket
from typing import Any, Callable, Iterator, List, Optional, Sequence

from .connection import Connection
from .errors import E_CLOSED, SonaraError
from .engines import Engines
from .extensions import Agent, Channels, System

ALL_EVENTS = ("state", "items", "log")


class Subscription:
    """Events on a connection of their own: iterate it, or ``read()`` with a
    timeout. Each event is a dict whose ``event`` is ``state``, ``item`` or
    ``log``. Iteration ends when the runtime closes the connection."""

    def __init__(self, conn: Connection):
        self._conn = conn

    def __iter__(self) -> Iterator[dict]:
        return self

    def __next__(self) -> dict:
        event = self._conn.next_event(None)
        if event is None:
            raise StopIteration
        return event

    def read(self, timeout: Optional[float] = None) -> Optional[dict]:
        """The next event, or None when none arrived within ``timeout``
        seconds. SonaraError ``E_CLOSED`` once the stream ended."""
        try:
            event = self._conn.next_event(timeout)
        except socket.timeout:
            return None
        if event is None:
            raise SonaraError(E_CLOSED, "the event stream ended")
        return event

    def close(self) -> None:
        self._conn.close()

    def __enter__(self) -> "Subscription":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()


class Client:
    """A connected Sonara client (see ``connect``). Thread-safe: requests on
    the shared connection run one at a time."""

    def __init__(self, conn: Connection, runtime: dict, info: dict, dial: Callable[[], Connection]):
        self._conn = conn
        self._dial = dial
        self._subs: List[Subscription] = []
        #: ``runtime.json`` of the instance this client talks to.
        self.runtime = runtime
        #: The ``hello`` reply: version, protocol, capabilities, extensions, unavailable.
        self.info = info
        self.channels = Channels(self.request)
        self.agent = Agent(self.request)
        self.system = System(self.request)
        self.engines = Engines(self.request)

    @property
    def closed(self) -> bool:
        return self._conn.closed

    def request(self, type_: str, fields: Optional[dict] = None) -> dict:
        """Send any protocol message; the ``ok: true`` reply."""
        return self._conn.request(type_, fields)

    def speak(self, text: str, mode: Optional[str] = None, interrupt: Optional[bool] = None,
              label: Optional[str] = None) -> int:
        """Add ``text`` as one item; its item id. ``mode``: ``append``
        (default) or ``replace``; ``interrupt`` also cuts the current item."""
        fields: dict = {"text": text}
        if mode is not None:
            fields["mode"] = mode
        if interrupt is not None:
            fields["interrupt"] = interrupt
        if label is not None:
            fields["label"] = label
        return self.request("speak", fields)["item_id"]

    def control(self, action: str) -> None:
        """play, pause, toggle, stop, skip, previous, next, restart, mute, unmute."""
        self.request("control", {"action": action})

    def set(self, key: str, value: Any) -> Any:
        """Change ``volume``, ``rate``, ``voice`` or ``engine``; the value now in force."""
        return self.request("set", {"key": key, "value": value})["value"]

    def get(self, key: str) -> Any:
        """Read a setting."""
        return self.request("get", {"key": key})["value"]

    def voices(self, engine: Optional[str] = None, refresh: bool = False) -> list:
        """Voices of one engine, or of all. ``refresh`` asks an external
        engine's provider again (protocol 1.2)."""
        fields: dict = {} if engine is None else {"engine": engine}
        if refresh:
            fields["refresh"] = True
        return self.request("voices", fields)["voices"]

    def subscribe(self, events: Sequence[str] = ALL_EVENTS) -> Subscription:
        """Open an event stream (``state``, ``items``, ``log``) on a new
        connection. When ``state`` is included the first event is the
        current state."""
        conn = self._dial()
        try:
            conn.request("subscribe", {"events": list(events)})
        except SonaraError:
            conn.close()
            raise
        sub = Subscription(conn)
        self._subs.append(sub)
        return sub

    def close(self) -> None:
        """Close this client's connections. The runtime keeps running for
        other clients and exits on its own when idle."""
        for sub in self._subs:
            sub.close()
        self._conn.close()

    def __enter__(self) -> "Client":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()
