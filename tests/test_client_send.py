import json
import socket
import threading

from echo import paths
from echo.client import send
from echo.protocol import PROTOCOL_VERSION, encode


def _echo_server(sock_path, ready, captured):
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(sock_path)
    srv.listen(1)
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
    sock_path = str(tmp_path / "speechd.sock")
    monkeypatch.setattr(paths, "SOCKET_PATH", sock_path, raising=False)
    import echo.client as client_mod
    monkeypatch.setattr(client_mod, "SOCKET_PATH", sock_path, raising=False)

    ready = threading.Event()
    captured = {}
    t = threading.Thread(target=_echo_server, args=(sock_path, ready, captured), daemon=True)
    t.start()
    assert ready.wait(2.0)

    msg = {"v": PROTOCOL_VERSION, "type": "ping"}
    result = send(msg, expect_reply=False)
    assert result is None
    t.join(timeout=2.0)
    assert captured["recv"] == msg


def test_send_round_trip_reply(tmp_path, monkeypatch):
    sock_path = str(tmp_path / "speechd.sock")
    import echo.client as client_mod
    monkeypatch.setattr(client_mod, "SOCKET_PATH", sock_path, raising=False)

    ready = threading.Event()
    captured = {}
    t = threading.Thread(target=_echo_server, args=(sock_path, ready, captured), daemon=True)
    t.start()
    assert ready.wait(2.0)

    reply = send({"v": PROTOCOL_VERSION, "type": "ping"}, expect_reply=True, timeout=2.0)
    assert reply == {"ok": True, "pong": "yes"}
    t.join(timeout=2.0)
