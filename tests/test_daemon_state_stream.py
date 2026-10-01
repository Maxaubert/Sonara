"""SUBSCRIBE state stream and the STATUS snapshot (#143).

An embedded player keeps one connection open and gets the daemon's state
pushed: on subscribe, then whenever it changes (after a handled message, and
when an utterance starts or ends). Socket writes happen on the subscriber's
own thread, never under the daemon lock, and a subscriber that cannot keep
up is dropped instead of slowing the daemon down."""
from __future__ import annotations

import json
import queue
import socket
import threading
import time

import pytest

from sonara.daemon import state_stream
from sonara.daemon.server import ConnectionServer
from sonara.protocol import MsgType, PROTOCOL_VERSION, encode
from sonara.queue import SpeechItem
from sonara.router import CONTROL
from tests.daemon_helpers import make_daemon

SNAPSHOT_KEYS = {"seq", "now_playing", "queue", "paused", "mute_level",
                 "volume", "summary_mode"}


def _msg(t, **kw):
    out = {"v": PROTOCOL_VERSION, "type": t}
    out.update(kw)
    return out


def _drain(sub):
    out = []
    while True:
        try:
            out.append(sub.q.get_nowait())
        except queue.Empty:
            return out


@pytest.fixture()
def pair():
    a, b = socket.socketpair()
    yield a, b
    for s in (a, b):
        try:
            s.close()
        except OSError:
            pass


# --- STATUS --------------------------------------------------------------

def test_status_returns_the_snapshot_and_keeps_its_old_fields():
    daemon, *_ = make_daemon(foreground="fg")
    status = daemon.handle_message(_msg(MsgType.STATUS))
    assert SNAPSHOT_KEYS <= set(status)
    # webui / CLI compatibility: the pre-#143 fields are still there.
    assert {"verbosity", "rate", "voice", "foreground", "minqueue"} <= set(status)
    assert status["now_playing"] is None
    assert status["queue"] == 0
    assert status["paused"] is False
    assert status["mute_level"] == 0
    assert status["volume"] == 100
    assert "type" not in status


def test_snapshot_now_playing_carries_session_tab_kind_text():
    daemon, *_ = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.SET_FOREGROUND, session="s1",
                               host_tab="tab-1"))
    daemon._current_item = SpeechItem(id=5, session="s1", kind="summary",
                                      text="Hello.", is_decision=False)
    status = daemon.handle_message(_msg(MsgType.STATUS))
    assert status["now_playing"] == {"session": "s1", "tab": "tab-1",
                                     "kind": "summary", "text": "Hello."}


def test_snapshot_control_cue_has_no_session():
    daemon, *_ = make_daemon(foreground="fg")
    daemon._current_item = SpeechItem(id=5, session=CONTROL, kind="prose",
                                      text="Paused.", is_decision=False)
    status = daemon.handle_message(_msg(MsgType.STATUS))
    assert status["now_playing"]["session"] is None
    assert status["now_playing"]["tab"] is None


def test_snapshot_hides_a_session_change_announcement():
    # "Session changed: X." is a cue, not content: a player must not show
    # it as now playing, whichever path spoke it.
    daemon, *_ = make_daemon(foreground="fg")
    daemon._current_item = SpeechItem(id=5, session="s1",
                                      kind="session_change",
                                      text="Session changed: s1.",
                                      is_decision=False)
    status = daemon.handle_message(_msg(MsgType.STATUS))
    assert status["now_playing"] is None


def test_snapshot_queue_counts_session_items_not_control_cues():
    daemon, *_ = make_daemon(foreground="fg")
    daemon._enqueue("a", "prose", "one", False)
    daemon._enqueue("b", "prose", "two", False)
    daemon._enqueue(CONTROL, "prose", "cue", False)
    assert daemon.handle_message(_msg(MsgType.STATUS))["queue"] == 2


def test_snapshot_reflects_pause_mute_volume_and_summary_mode():
    daemon, *_ = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PAUSE))
    daemon.handle_message(_msg(MsgType.MUTE))
    daemon.handle_message(_msg(MsgType.SET_VOLUME, volume=150))
    daemon.handle_message(_msg(MsgType.SET_SUMMARY_MODE, enabled=False))
    s = daemon.handle_message(_msg(MsgType.STATUS))
    assert (s["paused"], s["mute_level"], s["volume"], s["summary_mode"]) == (
        True, 1, 150, False)


# --- publish on change only ------------------------------------------------

def test_subscriber_gets_the_current_state_first(pair):
    daemon, *_ = make_daemon(foreground="fg")
    with daemon._lock:
        sub = daemon._state.add(pair[0])
    first = _drain(sub)
    assert len(first) == 1
    assert first[0]["type"] == "state"
    assert SNAPSHOT_KEYS <= set(first[0])


def test_state_is_pushed_on_change_only(pair):
    daemon, *_ = make_daemon(foreground="fg")
    with daemon._lock:
        sub = daemon._state.add(pair[0])
    seq0 = _drain(sub)[0]["seq"]
    daemon.handle_message(_msg(MsgType.PING))          # changes nothing
    daemon.handle_message(_msg(MsgType.STATUS))
    assert _drain(sub) == []
    daemon.handle_message(_msg(MsgType.PAUSE))
    events = _drain(sub)
    assert [e["paused"] for e in events] == [True]
    assert events[0]["seq"] == seq0 + 1


def test_speak_start_and_end_are_pushed(pair):
    daemon, _queue, speaker, *_ = make_daemon(foreground="fg")
    with daemon._lock:
        sub = daemon._state.add(pair[0])
    _drain(sub)
    daemon.handle_message(_msg(MsgType.SPEAK, text="Hello there.",
                               source="prism", tab="t1"))
    queued = _drain(sub)
    assert queued[-1]["queue"] == 1 and queued[-1]["now_playing"] is None
    during = []
    real_speak = speaker.speak

    def speak(text, **kw):
        during.extend(_drain(sub))           # what a player saw at speech start
        return real_speak(text, **kw)

    speaker.speak = speak
    daemon._playback.run_once()
    assert during[-1]["now_playing"] == {"session": "prism:t1", "tab": "t1",
                                         "kind": "summary",
                                         "text": "Hello there."}
    assert during[-1]["queue"] == 0
    after = _drain(sub)
    assert after[-1]["now_playing"] is None


def test_slow_subscriber_is_dropped_without_blocking(pair):
    daemon, *_ = make_daemon(foreground="fg")
    stream = state_stream.StateStream(lambda: state_stream.snapshot(daemon),
                                      queue_size=2)
    with daemon._lock:
        sub = stream.add(pair[0])        # 1 queued (the current state)
    for vol in (110, 120, 130):          # never drained: the queue overflows
        daemon.config["volume"] = vol
        t0 = time.monotonic()
        with daemon._lock:
            stream.publish()
        assert time.monotonic() - t0 < 0.5
    assert sub.dead.is_set()
    assert stream.count() == 0


def test_subscriber_cap():
    daemon, *_ = make_daemon(foreground="fg")
    socks = [socket.socketpair() for _ in range(state_stream.MAX_SUBSCRIBERS + 1)]
    try:
        with daemon._lock:
            subs = [daemon._state.add(a) for a, _b in socks]
        assert all(s is not None for s in subs[:-1])
        assert subs[-1] is None
    finally:
        for a, b in socks:
            a.close()
            b.close()


# --- over the real socket server --------------------------------------------

class _Reader:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""

    def event(self, timeout=3.0):
        """The next pushed event, or None once the daemon closed."""
        self.sock.settimeout(timeout)
        while b"\n" not in self.buf:
            try:
                data = self.sock.recv(4096)
            except ConnectionError:
                return None
            if not data:
                return None
            self.buf += data
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line)


class _Server:
    """The daemon's ConnectionServer on a loopback port."""

    def __init__(self, daemon):
        self.daemon = daemon
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.bind(("127.0.0.1", 0))
        srv.listen(16)
        self.port = srv.getsockname()[1]
        daemon._server.sock = srv
        daemon._server.token = "tok"
        daemon._running.set()
        self.thread = threading.Thread(target=daemon._server.accept_loop,
                                       daemon=True)
        self.thread.start()

    def connect(self):
        s = socket.create_connection(("127.0.0.1", self.port), timeout=3.0)
        s.sendall(b"tok\n")
        return s

    def send(self, msg, reply=False):
        s = self.connect()
        try:
            s.sendall(encode(msg))
            return _Reader(s).event() if reply else None
        finally:
            s.close()

    def close(self):
        self.daemon._running.clear()
        self.daemon._state.close_all()
        self.daemon._server.close()
        self.thread.join(timeout=3.0)


@pytest.fixture()
def server():
    daemon, *_ = make_daemon(foreground="fg")
    s = _Server(daemon)
    yield s
    s.close()


def _subscribe(server, events=("state",)):
    sock = server.connect()
    sock.sendall(encode(_msg(MsgType.SUBSCRIBE, events=list(events))))
    reader = _Reader(sock)
    return sock, reader, reader.event()


def _wait_for(pred, timeout=3.0):
    deadline = time.monotonic() + timeout
    while not pred() and time.monotonic() < deadline:
        time.sleep(0.02)
    return pred()


def test_subscribe_over_the_socket_streams_changes(server):
    sock, reader, first = _subscribe(server)
    try:
        assert first["type"] == "state" and first["paused"] is False
        server.send(_msg(MsgType.PAUSE))
        ev = reader.event()
        assert ev["type"] == "state" and ev["paused"] is True
        assert ev["seq"] > first["seq"]
    finally:
        sock.close()


def test_subscriber_is_exempt_from_the_read_timeout(server, monkeypatch):
    monkeypatch.setattr(ConnectionServer, "READ_TIMEOUT_S", 0.2)
    sock, reader, _first = _subscribe(server)
    try:
        time.sleep(0.6)                 # well past the read timeout
        server.send(_msg(MsgType.MUTE))
        assert reader.event()["mute_level"] == 1
    finally:
        sock.close()


def test_subscribers_do_not_take_connection_slots(server):
    server.daemon._server.conn_sem = threading.BoundedSemaphore(1)
    sock, _reader, _first = _subscribe(server)
    try:
        # The only connection slot is free again: a request still works.
        assert _wait_for(lambda: server.daemon._state.count() == 1)
        status = server.send(_msg(MsgType.STATUS), reply=True)
        assert status is not None and "seq" in status
    finally:
        sock.close()


def test_subscribe_beyond_the_cap_is_refused(server):
    server.daemon._state._max = 1
    sock1, _r1, _f1 = _subscribe(server)
    sock2, reader2, refused = _subscribe(server)
    try:
        assert refused["type"] == "error"
        assert reader2.event() is None      # and closed
    finally:
        sock1.close()
        sock2.close()


def test_unknown_event_kind_is_refused(server):
    sock, reader, refused = _subscribe(server, events=("nope",))
    try:
        assert refused["type"] == "error"
        assert reader.event() is None
    finally:
        sock.close()


def test_a_closed_subscriber_frees_its_slot(server, monkeypatch):
    monkeypatch.setattr(state_stream, "POLL_S", 0.05)
    sock, _reader, _first = _subscribe(server)
    assert _wait_for(lambda: server.daemon._state.count() == 1)
    sock.close()
    assert _wait_for(lambda: server.daemon._state.count() == 0)


def test_shutdown_closes_subscribers(server):
    sock, reader, _first = _subscribe(server)
    try:
        server.daemon._state.close_all()
        assert reader.event() is None
    finally:
        sock.close()


def test_client_subscribe_generator(server, tmp_path, monkeypatch):
    from sonara import client
    from sonara.platform import transport
    lock_path = tmp_path / "daemon.lock"
    transport.write_lockfile(lock_path, "127.0.0.1", server.port, "tok", 1)
    monkeypatch.setattr(client, "LOCK_PATH", lock_path)
    gen = client.subscribe()
    try:
        first = next(gen)
        assert first["type"] == "state"
        server.send(_msg(MsgType.PAUSE))
        assert next(gen)["paused"] is True
    finally:
        gen.close()
