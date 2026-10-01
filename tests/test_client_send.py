import json
import socket
import threading

from sonara import paths
from sonara.client import send
from sonara.platform import transport
from sonara.protocol import PROTOCOL_VERSION, encode


def _reply_server(lock_path, ready, captured, token="tok"):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    transport.write_lockfile(lock_path, "127.0.0.1", srv.getsockname()[1], token, 1)
    ready.set()
    conn, _ = srv.accept()
    try:
        with conn:
            buf = b""
            while b"\n" not in buf:
                try:
                    data = conn.recv(4096)
                except OSError:
                    break
                if not data:
                    break
                buf += data
            # strip the token handshake line; the payload is the second line
            token_line, _, buf = buf.partition(b"\n")
            captured["token"] = token_line.decode("utf-8")
            while b"\n" not in buf:
                try:
                    data = conn.recv(4096)
                except OSError:
                    break
                if not data:
                    break
                buf += data
            if buf:
                line = buf.split(b"\n", 1)[0]
                captured["recv"] = json.loads(line)
            try:
                conn.sendall(encode({"ok": True, "pong": "yes"}))
            except OSError:
                # Client closed without reading (e.g. expect_reply=False); ignore.
                pass
    finally:
        srv.close()


def test_send_no_reply(tmp_path, monkeypatch):
    lock_path = tmp_path / "daemon.lock"
    monkeypatch.setattr(paths, "LOCK_PATH", lock_path, raising=False)
    import sonara.client as client_mod
    monkeypatch.setattr(client_mod, "LOCK_PATH", lock_path, raising=False)

    ready = threading.Event()
    captured = {}
    t = threading.Thread(target=_reply_server, args=(lock_path, ready, captured), daemon=True)
    t.start()
    assert ready.wait(2.0)

    msg = {"v": PROTOCOL_VERSION, "type": "ping"}
    result = send(msg, expect_reply=False)
    assert result is None
    t.join(timeout=2.0)
    assert captured["token"] == "tok"
    assert captured["recv"] == msg


def test_send_round_trip_reply(tmp_path, monkeypatch):
    lock_path = tmp_path / "daemon.lock"
    import sonara.client as client_mod
    monkeypatch.setattr(client_mod, "LOCK_PATH", lock_path, raising=False)

    ready = threading.Event()
    captured = {}
    t = threading.Thread(target=_reply_server, args=(lock_path, ready, captured), daemon=True)
    t.start()
    assert ready.wait(2.0)

    reply = send({"v": PROTOCOL_VERSION, "type": "ping"}, expect_reply=True, timeout=2.0)
    assert reply == {"ok": True, "pong": "yes"}
    t.join(timeout=2.0)


def _batch_server(lock_path, ready, captured, token="tok"):
    """Accept connections until the listener closes; record each one's lines."""
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(4)
    srv.settimeout(1.0)
    transport.write_lockfile(lock_path, "127.0.0.1", srv.getsockname()[1], token, 1)
    ready.set()
    conns = captured.setdefault("conns", [])
    try:
        while True:
            try:
                conn, _ = srv.accept()
            except OSError:
                return              # no further connection within the window
            with conn:
                buf = b""
                while True:
                    data = conn.recv(4096)
                    if not data:
                        break
                    buf += data
            conns.append([ln.decode("utf-8") for ln in buf.split(b"\n") if ln])
    finally:
        srv.close()


def test_send_many_sends_one_event_over_one_connection_in_order(tmp_path, monkeypatch):
    # Architecture review 0.5 (#137): one connection per message let the
    # daemon's per-connection threads apply SET_FOREGROUND/FLUSH (or
    # EARCON/CHOICE) of a single hook event in either order.
    from sonara.client import send_many
    lock_path = tmp_path / "daemon.lock"
    import sonara.client as client_mod
    monkeypatch.setattr(client_mod, "LOCK_PATH", lock_path, raising=False)
    ready = threading.Event()
    captured = {}
    t = threading.Thread(target=_batch_server, args=(lock_path, ready, captured),
                         daemon=True)
    t.start()
    assert ready.wait(2.0)
    msgs = [{"v": PROTOCOL_VERSION, "type": "set_foreground", "session": "s"},
            {"v": PROTOCOL_VERSION, "type": "flush", "session": "s"}]
    send_many(msgs)
    t.join(timeout=3.0)
    assert len(captured["conns"]) == 1
    lines = captured["conns"][0]
    assert lines[0] == "tok"
    assert [json.loads(x) for x in lines[1:]] == msgs


def test_send_many_with_nothing_opens_no_connection(tmp_path, monkeypatch):
    from sonara.client import send_many
    import sonara.client as client_mod
    monkeypatch.setattr(client_mod, "LOCK_PATH", tmp_path / "missing.lock",
                        raising=False)
    send_many([])                   # no lockfile needed: nothing to send
