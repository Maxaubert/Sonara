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
