"""The state stream for embedded players (#143): SUBSCRIBE and the STATUS
snapshot.

snapshot() reads what a player shows (the item being spoken, the queue
length, pause, mute, volume, summary mode) under the daemon lock.
StateStream.publish() runs after every handled message and around every
utterance; it compares the snapshot with the last one and, only when it
changed, bumps seq and offers the event to every subscriber.

Each subscriber has a small queue and its own thread (the connection's
handler thread) that writes to the socket, so no socket write ever happens
under the daemon lock. A subscriber whose queue is full is too slow and is
dropped on the spot; a closed peer is noticed within POLL_S."""
from __future__ import annotations

import queue
import select
import socket
import sys
import threading

from sonara import config_schema
from sonara.protocol import EventType, encode
from sonara.router import CONTROL

# Subscribers have their own cap, apart from the connection-thread cap: a
# player holds its connection for the daemon's lifetime.
MAX_SUBSCRIBERS = 4
# Events waiting for one subscriber. A burst of changes fits easily; a
# subscriber that falls this far behind is not reading and is dropped.
QUEUE_SIZE = 32
# How often an idle subscriber thread checks that its peer is still there.
POLL_S = 1.0
# A write that cannot complete in this long means a stuck peer.
WRITE_TIMEOUT_S = 5.0


def snapshot(daemon) -> dict:
    """What an embedded player shows. Caller holds the daemon lock."""
    d = daemon
    item = d._current_item
    now_playing = None
    if item is not None:
        sid = item.session if item.session and item.session != CONTROL else None
        now_playing = {
            "session": sid,
            "tab": d.sessions.host_tab(sid) if sid else None,
            "kind": item.kind,
            "text": item.text,
        }
    return {
        "now_playing": now_playing,
        "queue": sum(ch.pending() for s, ch in d.router.channels.items()
                     if s != CONTROL),
        "paused": d._paused.is_set(),
        "mute_level": int(d._mute_level),
        "volume": int(config_schema.current(d.config, "volume")),
        "summary_mode": bool(config_schema.current(d.config, "summary_mode")),
    }


class Subscriber:
    """One subscribed connection: its socket and its pending events."""

    def __init__(self, conn, queue_size: int) -> None:
        self.conn = conn
        self.q: "queue.Queue[dict]" = queue.Queue(maxsize=queue_size)
        self.dead = threading.Event()

    def offer(self, event: dict) -> bool:
        """Queue *event* without blocking. A full queue drops the
        subscriber. Returns False once the subscriber is dropped."""
        if self.dead.is_set():
            return False
        try:
            self.q.put_nowait(event)
            return True
        except queue.Full:
            self.drop()
            return False

    def drop(self) -> None:
        """Mark dead and shut the socket, which ends a blocked write on the
        subscriber's thread. Never blocks."""
        self.dead.set()
        try:
            self.conn.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass


class StateStream:
    """The subscriber list and the change detection. *snapshot_fn* returns
    the current snapshot and is called with the daemon lock held: publish,
    current and add all run under it. The subscriber list has its own small
    lock, taken inside the daemon lock or alone, never the other way round."""

    def __init__(self, snapshot_fn, max_subscribers: "int | None" = None,
                 queue_size: int = QUEUE_SIZE) -> None:
        self._snapshot = snapshot_fn
        self._max = MAX_SUBSCRIBERS if max_subscribers is None else max_subscribers
        self._queue_size = queue_size
        self._seq = 0
        self._last = None
        self._subs: list = []
        self._subs_lock = threading.Lock()

    def _event(self) -> dict:
        out = {"type": EventType.STATE, "seq": self._seq}
        out.update(self._last)
        return out

    def publish(self) -> bool:
        """Offer the snapshot to every subscriber if it changed since the
        last publish. Caller holds the daemon lock. Returns True on a
        change."""
        snap = self._snapshot()
        if snap == self._last:
            return False
        self._last = snap
        self._seq += 1
        event = self._event()
        for sub in self._subscribers():
            if not sub.offer(event):
                self._remove(sub)
        return True

    def current(self) -> dict:
        """The up-to-date state event (publishing a pending change first).
        Caller holds the daemon lock."""
        self.publish()
        return self._event()

    def add(self, conn) -> "Subscriber | None":
        """Register *conn* as a subscriber and queue the current state as
        its first event. None when the subscriber cap is reached. Caller
        holds the daemon lock."""
        if self.count() >= self._max:
            return None
        # Publish a pending change to the others BEFORE joining, or the new
        # subscriber would get that event twice.
        first = self.current()
        sub = Subscriber(conn, self._queue_size)
        with self._subs_lock:
            if len(self._subs) >= self._max:
                return None
            self._subs.append(sub)
        sub.offer(first)
        return sub

    def count(self) -> int:
        with self._subs_lock:
            return len(self._subs)

    def _subscribers(self) -> list:
        with self._subs_lock:
            return list(self._subs)

    def _remove(self, sub) -> None:
        with self._subs_lock:
            if sub in self._subs:
                self._subs.remove(sub)

    def close_all(self) -> None:
        """Drop every subscriber (daemon shutdown): their threads end."""
        for sub in self._subscribers():
            sub.drop()
            self._remove(sub)

    def serve(self, sub, running) -> None:
        """Write *sub*'s events to its socket until the daemon stops, the
        peer goes away or the subscriber is dropped. Runs on the
        connection's own thread, without the daemon lock."""
        conn = sub.conn
        try:
            conn.settimeout(WRITE_TIMEOUT_S)
            while running.is_set() and not sub.dead.is_set():
                try:
                    event = sub.q.get(timeout=POLL_S)
                except queue.Empty:
                    if _peer_closed(conn):
                        return
                    continue
                try:
                    conn.sendall(encode(event))
                except OSError:
                    return
        except Exception:  # noqa: BLE001 - one subscriber must never hurt the daemon
            import traceback
            traceback.print_exc(file=sys.stderr)
        finally:
            sub.dead.set()
            self._remove(sub)


def _peer_closed(conn) -> bool:
    """True when the peer closed the connection. A subscriber sends nothing
    after SUBSCRIBE; any bytes it does send are read and ignored."""
    try:
        readable, _w, _x = select.select([conn], [], [], 0)
        if not readable:
            return False
        return not conn.recv(4096)
    except (OSError, ValueError):
        return True
