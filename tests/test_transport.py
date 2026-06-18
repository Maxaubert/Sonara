import socket, threading
from sonari.platform import transport


def test_write_then_read_lockfile_roundtrips(tmp_path):
    lock = tmp_path / "daemon.lock"
    transport.write_lockfile(lock, "127.0.0.1", 54321, "deadbeef", 4242)
    info = transport.read_lockfile(lock)
    assert info == {"host": "127.0.0.1", "port": 54321,
                    "token": "deadbeef", "pid": 4242}
    assert oct(lock.stat().st_mode)[-3:] == "600"


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
    # socket actually accepts — it returns True because connect() succeeds.
    t = threading.Thread(target=lambda: srv.accept(), daemon=True)
    t.start()
    assert transport.connectable(lock) is True
    srv.close()


def test_connectable_false_when_lockfile_absent(tmp_path):
    assert transport.connectable(tmp_path / "absent.lock") is False


def test_acquire_singleton_is_exclusive(tmp_path):
    # Restores the single-instance guarantee AF_UNIX's fixed-path bind gave us:
    # only one holder at a time; releasing lets the next acquire succeed.
    lock = tmp_path / "daemon.singleton"
    f1 = transport.acquire_singleton(lock)
    assert f1 is not None, "first acquire should win"
    assert transport.acquire_singleton(lock) is None, "second acquire must fail while held"
    f1.close()  # releases the flock
    f2 = transport.acquire_singleton(lock)
    assert f2 is not None, "after release, acquire should win again"
    f2.close()


def test_acquire_singleton_windows_branch(tmp_path, monkeypatch):
    import sonari.platform.transport as tr
    monkeypatch.setattr(tr.sys, "platform", "win32")
    lock = tmp_path / "daemon.singleton"
    f1 = tr.acquire_singleton(lock)
    assert f1 is not None
    assert tr.acquire_singleton(lock) is None   # msvcrt fake: 2nd lock on same fd-id fails
    f1.close()
