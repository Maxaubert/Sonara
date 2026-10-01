import os, socket, threading
from unittest import mock

import pytest

from sonara.platform import transport


def test_write_then_read_lockfile_roundtrips(tmp_path):
    lock = tmp_path / "daemon.lock"
    transport.write_lockfile(lock, "127.0.0.1", 54321, "deadbeef", 4242)
    info = transport.read_lockfile(lock)
    assert info == {"host": "127.0.0.1", "port": 54321,
                    "token": "deadbeef", "pid": 4242}


@pytest.mark.skipif(os.name == "nt",
                    reason="Windows chmod only honours the read-only bit, so the "
                           "mode reads 666; the owner-only guarantee comes from "
                           "the %USERPROFILE% ACL instead. Intent is covered by "
                           "test_write_lockfile_requests_owner_only_mode.")
def test_write_lockfile_is_owner_only_on_posix(tmp_path):
    lock = tmp_path / "daemon.lock"
    transport.write_lockfile(lock, "127.0.0.1", 54321, "deadbeef", 4242)
    assert oct(lock.stat().st_mode)[-3:] == "600"


def test_write_lockfile_requests_owner_only_mode(tmp_path):
    """The lockfile carries the daemon's auth token, so write_lockfile must ask
    for 0o600 on every platform. Windows cannot honour it (see the skip above),
    which is exactly why the REQUEST is asserted separately from the resulting
    mode -- otherwise the only check of this intent silently vanishes on the
    one OS Sonara actually ships to."""
    lock = tmp_path / "daemon.lock"
    with mock.patch("os.chmod", wraps=os.chmod) as chmod:
        transport.write_lockfile(lock, "127.0.0.1", 54321, "deadbeef", 4242)
    assert [c.args[1] for c in chmod.call_args_list] == [0o600]


def test_read_lockfile_missing_returns_none(tmp_path):
    assert transport.read_lockfile(tmp_path / "absent.lock") is None


def test_connectable_true_against_a_live_listener(tmp_path):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    port = srv.getsockname()[1]
    lock = tmp_path / "daemon.lock"
    transport.write_lockfile(lock, "127.0.0.1", port, "tok", 999999)
    # PID 999999 is unlikely-live; connectable must NOT depend on PID when the
    # socket actually accepts -- it returns True because connect() succeeds.
    t = threading.Thread(target=lambda: srv.accept(), daemon=True)
    t.start()
    assert transport.connectable(lock) is True
    srv.close()


def test_connectable_false_when_lockfile_absent(tmp_path):
    assert transport.connectable(tmp_path / "absent.lock") is False


def test_write_lockfile_optional_http_port(tmp_path):
    from sonara.platform import transport
    p = tmp_path / "lock"
    transport.write_lockfile(p, "127.0.0.1", 5000, "tok", 42)
    assert "http_port" not in transport.read_lockfile(p)
    transport.write_lockfile(p, "127.0.0.1", 5000, "tok", 42, http_port=27431)
    assert transport.read_lockfile(p)["http_port"] == 27431


def test_write_lockfile_retries_a_replace_denied_by_a_reader(tmp_path, monkeypatch):
    # E20: on Windows a hook reading the old lockfile blocks os.replace for a
    # moment (PermissionError); the daemon startup used to die on it.
    real_replace = os.replace
    denied = {"n": 0}

    def flaky(src, dst):
        if denied["n"] < 2:
            denied["n"] += 1
            raise PermissionError(5, "Access is denied")
        return real_replace(src, dst)

    monkeypatch.setattr(transport.os, "replace", flaky)
    monkeypatch.setattr(transport.time, "sleep", lambda s: None)
    p = tmp_path / "daemon.lock"
    transport.write_lockfile(p, "127.0.0.1", 5, "tok", 9)
    assert transport.read_lockfile(p)["port"] == 5
    assert denied["n"] == 2


def test_read_lockfile_retries_a_transient_sharing_violation(tmp_path, monkeypatch):
    p = tmp_path / "daemon.lock"
    transport.write_lockfile(p, "127.0.0.1", 7, "tok", 9)
    real_open = open
    denied = {"n": 0}

    def flaky_open(path, *a, **k):
        if str(path) == str(p) and denied["n"] < 1:
            denied["n"] += 1
            raise PermissionError(32, "being used by another process")
        return real_open(path, *a, **k)

    monkeypatch.setattr("builtins.open", flaky_open)
    monkeypatch.setattr(transport.time, "sleep", lambda s: None)
    assert transport.read_lockfile(p)["port"] == 7
