"""E18: the CLI prints a clear error, not a traceback, when the daemon hangs
or its lockfile is damaged; and `sonara settings` tells "not running" apart
from "running without a settings page"."""
from __future__ import annotations

import json
import socket
import threading

import pytest

from sonara import cli, client, paths
from sonara.platform import transport


def _silent_server(lock_path, stop):
    """Accepts and reads, never answers: a daemon stuck under its lock."""
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    transport.write_lockfile(lock_path, "127.0.0.1", srv.getsockname()[1], "tok", 1)

    def run():
        conn, _ = srv.accept()
        stop.wait(5)
        conn.close()
        srv.close()
    t = threading.Thread(target=run, daemon=True)
    t.start()
    return t


def test_send_to_a_hung_daemon_raises_unresponsive():
    stop = threading.Event()
    paths.ensure_sonara_dir()
    t = _silent_server(client.LOCK_PATH, stop)
    try:
        with pytest.raises(client.DaemonUnresponsive):
            client.send({"v": 1, "type": "status"}, expect_reply=True, timeout=0.3)
    finally:
        stop.set()
        t.join(5)


def test_status_against_a_hung_daemon_prints_an_error(monkeypatch, capsys):
    def hung(msg, expect_reply=False, timeout=2.0):
        raise client.DaemonUnresponsive("timed out")
    monkeypatch.setattr(client, "send", hung)
    assert cli.main(["status"]) == 1
    err = capsys.readouterr().err
    assert "not responding" in err and "sonara shutdown" in err


def test_a_lockfile_missing_keys_reads_as_not_running(capsys):
    paths.ensure_sonara_dir()
    client.LOCK_PATH.write_text(json.dumps({"pid": 1}), encoding="utf-8")
    with pytest.raises(client.DaemonNotRunning):
        client.send({"v": 1, "type": "status"}, expect_reply=True)


def test_connect_closes_the_socket_when_connect_fails(monkeypatch, tmp_path):
    lock = tmp_path / "daemon.lock"
    transport.write_lockfile(lock, "127.0.0.1", 1, "tok", 1)
    made = []

    class S:
        def __init__(self, *a):
            self.closed = False
            made.append(self)

        def settimeout(self, t):
            pass

        def connect(self, addr):
            raise ConnectionRefusedError("refused")

        def close(self):
            self.closed = True
    monkeypatch.setattr(transport.socket, "socket", S)
    with pytest.raises(OSError):
        transport.connect(lock)
    assert made and made[0].closed


def test_settings_when_the_page_did_not_start(monkeypatch, tmp_path, capsys):
    lock = tmp_path / "daemon.lock"
    lock.write_text(json.dumps({"host": "127.0.0.1", "port": 5000,
                                "token": "tok", "pid": 1}))
    monkeypatch.setattr(paths, "LOCK_PATH", lock)
    monkeypatch.setattr(paths, "socket_connectable", lambda: True)
    assert cli.main(["settings"]) == 1
    out = capsys.readouterr().out
    assert "not running" not in out
    assert "settings page" in out.lower() and "speechd.log" in out
