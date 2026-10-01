"""Connection-handling robustness: message-dispatch guard (#13) and the
concurrent-handler cap (#14)."""
from sonara.daemon import _MAX_CONN_THREADS
from tests.daemon_helpers import make_daemon


def test_handle_message_guarded_contains_exceptions():
    # #13: a message whose handling raises must NOT propagate -- that would kill
    # the connection thread silently. The guard logs and returns None.
    daemon, *_ = make_daemon(foreground="fg")

    def boom(msg):
        raise RuntimeError("bad message")

    daemon.handle_message = boom
    assert daemon._handle_message_guarded({"type": "whatever"}) is None


def test_spawn_conn_handler_drops_connection_at_capacity():
    # #14: with the concurrency cap exhausted, a new connection is dropped
    # (closed) rather than leaking another thread.
    daemon, *_ = make_daemon()
    for _ in range(_MAX_CONN_THREADS):
        assert daemon._conn_sem.acquire(blocking=False)
    closed = {"n": 0}

    class FakeConn:
        def close(self):
            closed["n"] += 1

    assert daemon._spawn_conn_handler(FakeConn()) is False
    assert closed["n"] == 1


def test_spawn_conn_handler_releases_permit_when_thread_start_fails(monkeypatch):
    # M8: if Thread.start() raises (e.g. the OS refuses a new thread), the permit
    # acquired for this connection must be released and the connection closed --
    # otherwise capacity bleeds a slot and the daemon eventually refuses everyone.
    daemon, *_ = make_daemon()

    class BoomThread:
        def __init__(self, *a, **k):
            pass

        def start(self):
            raise RuntimeError("cannot create thread")

    monkeypatch.setattr("threading.Thread", BoomThread)
    closed = {"n": 0}

    class FakeConn:
        def close(self):
            closed["n"] += 1

    assert daemon._spawn_conn_handler(FakeConn()) is False
    assert closed["n"] == 1
    # All permits are free again (none leaked).
    n = 0
    while daemon._conn_sem.acquire(blocking=False):
        n += 1
    assert n == _MAX_CONN_THREADS


def test_handle_conn_guarded_releases_permit_even_on_error():
    # The permit is released even if _handle_conn raises, so capacity recovers
    # instead of bleeding a slot on every failed connection.
    daemon, *_ = make_daemon()

    def raising(conn):
        raise RuntimeError("handler blew up")

    daemon._handle_conn = raising
    daemon._conn_sem.acquire()              # the permit a spawn would have taken
    daemon._handle_conn_guarded(object())   # raises inside; finally must release
    n = 0
    while daemon._conn_sem.acquire(blocking=False):
        n += 1
    assert n == _MAX_CONN_THREADS           # capacity fully restored


def test_messages_on_one_connection_are_handled_in_order():
    # Architecture review 0.5 (#137): client.send_many writes all messages of
    # one hook event on ONE connection; the daemon applies them sequentially
    # on that connection's thread, in the order sent.
    import socket
    import threading
    from sonara.protocol import encode
    daemon, *_ = make_daemon(foreground="fg")
    daemon._token = "tok"
    daemon._running.set()
    seen = []
    daemon.handle_message = lambda msg: seen.append(msg["type"])
    server_end, client_end = socket.socketpair()
    t = threading.Thread(target=daemon._handle_conn, args=(server_end,), daemon=True)
    t.start()
    client_end.sendall(b"tok\n" + b"".join(
        encode({"type": n}) for n in ("set_foreground", "flush", "earcon", "choice")))
    client_end.close()
    t.join(timeout=3.0)
    assert seen == ["set_foreground", "flush", "earcon", "choice"]


def test_single_message_connections_still_work():
    # Backward compatibility: the old one-message-per-connection client.
    import socket
    import threading
    from sonara.protocol import encode
    daemon, *_ = make_daemon(foreground="fg")
    daemon._token = "tok"
    daemon._running.set()
    seen = []
    daemon.handle_message = lambda msg: seen.append(msg["type"])
    for name in ("set_foreground", "flush"):
        server_end, client_end = socket.socketpair()
        t = threading.Thread(target=daemon._handle_conn, args=(server_end,),
                             daemon=True)
        t.start()
        client_end.sendall(b"tok\n" + encode({"type": name}))
        client_end.close()
        t.join(timeout=3.0)
    assert seen == ["set_foreground", "flush"]
