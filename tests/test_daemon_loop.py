import socket
import tempfile
import threading
import time

from sonari.queue import SpeechItem
from sonari.protocol import MsgType, encode, decode
from tests.daemon_helpers import make_daemon


def test_speak_loop_speaks_queued_item_then_stops():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose", text="hello world", is_decision=False))

    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    try:
        deadline = time.time() + 2.0
        while time.time() < deadline and not speaker.spoken:
            time.sleep(0.01)
        assert speaker.spoken == ["hello world"]
    finally:
        daemon.stop()
        t.join(timeout=2.0)
    assert not t.is_alive()


def test_speak_loop_survives_a_speaker_exception():
    # A single utterance that throws (e.g. the WinRT synth chokes on a chunk)
    # must NOT kill the speak thread; later items must still be spoken.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    spoken = []

    def boom(text, cancel_epoch=None):
        if text == "bad":
            raise RuntimeError("synth blew up")
        spoken.append(text)
        return True

    speaker.speak = boom
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose", text="bad", is_decision=False))
    queue.enqueue(SpeechItem(id=2, session="fg", kind="prose", text="good", is_decision=False))

    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    try:
        deadline = time.time() + 2.0
        while time.time() < deadline and "good" not in spoken:
            time.sleep(0.01)
        assert "good" in spoken, "speak loop died on the bad utterance"
    finally:
        daemon.stop()
        t.join(timeout=2.0)
    assert not t.is_alive()


def test_speak_loop_idles_when_queue_empty_then_stops():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    time.sleep(0.05)
    assert speaker.spoken == []
    daemon.stop()
    t.join(timeout=2.0)
    assert not t.is_alive()


def test_speak_loop_fifo_order_including_items_added_after_start():
    """Items already queued and items enqueued after the loop starts are spoken
    in FIFO order; the wake path (items added while the loop is idle) must also
    be handled correctly."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")

    # Pre-load two items before starting the loop.
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose", text="first", is_decision=False))
    queue.enqueue(SpeechItem(id=2, session="fg", kind="prose", text="second", is_decision=False))

    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()

    # Wait for the two pre-loaded items to be spoken.
    deadline = time.time() + 2.0
    while time.time() < deadline and len(speaker.spoken) < 2:
        time.sleep(0.01)

    # Now the loop should be idle (queue drained).  Enqueue a third item to
    # exercise the wake path: the loop must call _wake.wait(), we set() _wake
    # via _enqueue, and it must pick up the new item.
    queue.enqueue(SpeechItem(id=3, session="fg", kind="prose", text="third (wake)", is_decision=False))
    daemon._wake.set()  # simulate what _enqueue does

    deadline = time.time() + 2.0
    while time.time() < deadline and len(speaker.spoken) < 3:
        time.sleep(0.01)

    daemon.stop()
    t.join(timeout=2.0)
    assert not t.is_alive()

    assert speaker.spoken == ["first", "second", "third (wake)"]


def _make_inet_daemon(tmp_path):
    """Start a daemon with its accept + speak loops on a localhost TCP port.

    Returns (daemon, (host, port), [threads], speaker).  The daemon gates every
    connection on a token sent as the first newline-terminated line.  Caller must
    call daemon.stop() and join threads when done.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(4)
    host, port = srv.getsockname()
    daemon._server = srv
    daemon._token = "testtoken"          # daemon checks this as the first line
    daemon._running.set()

    speak_t = threading.Thread(target=daemon._speak_loop, daemon=True)
    accept_t = threading.Thread(target=daemon._accept_loop, daemon=True)
    speak_t.start()
    accept_t.start()

    return daemon, (host, port), [speak_t, accept_t], speaker


def test_handle_conn_ping_round_trip():
    """Connect to a live daemon over TCP, authenticate with the token, send PING,
    receive {ok: True}."""
    with tempfile.TemporaryDirectory() as tmp:
        daemon, (host, port), threads, speaker = _make_inet_daemon(tmp)
        try:
            # Give the accept loop a moment to start listening.
            time.sleep(0.05)

            client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            client.connect((host, port))
            client.settimeout(2.0)

            client.sendall(b"testtoken\n")           # token handshake first
            client.sendall(encode({"type": MsgType.PING}))

            buf = b""
            while b"\n" not in buf:
                buf += client.recv(4096)
            client.close()

            line = buf.split(b"\n")[0]
            reply = decode(line)
            assert reply == {"ok": True}
        finally:
            daemon.stop()
            for t in threads:
                t.join(timeout=2.0)


def test_handle_conn_status_round_trip():
    """Connect to a live daemon over TCP, authenticate, send STATUS, receive a
    dict with the expected keys."""
    with tempfile.TemporaryDirectory() as tmp:
        daemon, (host, port), threads, speaker = _make_inet_daemon(tmp)
        try:
            time.sleep(0.05)

            client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            client.connect((host, port))
            client.settimeout(2.0)

            client.sendall(b"testtoken\n")           # token handshake first
            client.sendall(encode({"type": MsgType.STATUS}))

            buf = b""
            while b"\n" not in buf:
                buf += client.recv(4096)
            client.close()

            line = buf.split(b"\n")[0]
            reply = decode(line)
            assert set(reply.keys()) >= {"verbosity", "rate", "voice", "foreground", "queue_len"}
            assert reply["queue_len"] == 0
        finally:
            daemon.stop()
            for t in threads:
                t.join(timeout=2.0)


def test_handle_conn_rejects_wrong_token():
    """A connection that sends the wrong token is dropped without a reply."""
    with tempfile.TemporaryDirectory() as tmp:
        daemon, (host, port), threads, speaker = _make_inet_daemon(tmp)
        try:
            time.sleep(0.05)

            client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            client.connect((host, port))
            client.settimeout(2.0)

            client.sendall(b"wrongtoken\n")          # bad token -> rejected
            try:
                client.sendall(encode({"type": MsgType.PING}))
            except OSError:
                pass  # peer already closed; the connection was rejected

            # The daemon drops the peer without replying: recv returns EOF, or the
            # socket has already been reset (no PING reply is ever delivered).
            try:
                data = client.recv(4096)
            except (ConnectionResetError, ConnectionAbortedError):
                # Windows may abort (WinError 10053) rather than reset when the
                # daemon drops the unauthenticated peer; both mean "rejected".
                data = b""
            client.close()
            assert data == b""
        finally:
            daemon.stop()
            for t in threads:
                t.join(timeout=2.0)
