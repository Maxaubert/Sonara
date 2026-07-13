"""Daemon lifecycle (#23): SHUTDOWN message + the stop sentinel that gates
every respawn path (supervisor loop, lazy start)."""
import os

from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def test_shutdown_replies_ok_then_arms_deferred_stop(monkeypatch):
    # The reply must reach the socket BEFORE teardown, so the handler replies
    # ok and defers stop() through a short timer (#23).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    import sonara.daemon as daemon_module
    armed = []

    class FakeTimer:
        def __init__(self, delay, fn, args=()):
            armed.append((delay, fn))
        def start(self):
            pass
        daemon = False
    monkeypatch.setattr(daemon_module.threading, "Timer", FakeTimer)
    reply = daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SHUTDOWN})
    assert reply == {"ok": True}
    assert armed and armed[0][1] == daemon.stop     # stop deferred, not inline


def test_stop_sentinel_blocks_lazy_start(monkeypatch, tmp_path):
    # ensure_running fires on EVERY hook event; without the sentinel gate a
    # shutdown was resurrected within seconds (#23).
    import sonara.daemon as daemon_module
    from sonara import paths
    sentinel = tmp_path / "stopped"
    sentinel.write_text("")
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", sentinel)
    monkeypatch.setattr(daemon_module, "socket_connectable", lambda: False)
    spawned = []
    monkeypatch.setattr(daemon_module.subprocess, "Popen",
                        lambda *a, **k: spawned.append(a))
    daemon_module.ensure_running()
    assert spawned == []                            # sentinel: no lazy spawn


def test_lazy_start_spawns_without_sentinel(monkeypatch, tmp_path):
    import sonara.daemon as daemon_module
    from sonara import paths
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", tmp_path / "stopped")
    monkeypatch.setattr(daemon_module, "socket_connectable", lambda: False)
    spawned = []
    monkeypatch.setattr(daemon_module.subprocess, "Popen",
                        lambda *a, **k: spawned.append(a) or None)

    class FakeSup:
        def launch_spec(self):
            return (["pythonw", "-m", "sonara.daemon"], {})

    class FakePlat:
        supervisor = FakeSup()
    monkeypatch.setattr("sonara.platform.get_platform", lambda: FakePlat())
    daemon_module.ensure_running()
    assert spawned                                   # no sentinel: spawns as before


def test_supervisor_loop_exits_when_sentinel_present(monkeypatch, tmp_path):
    # The supervisor loop must EXIT instead of respawning while the sentinel
    # exists; previously nothing could ever stop the respawn loop (#23).
    from sonara.platform.windows import supervisor_loop as sl
    from sonara import paths
    sentinel = tmp_path / "stopped"
    sentinel.write_text("")
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", sentinel)
    monkeypatch.setattr(sl, "launch_spec",
                        lambda pw: (_ for _ in ()).throw(AssertionError("must not spawn")))
    sl.run_supervisor_loop("pythonw.exe")            # returns instead of looping


def test_supervisor_stop_requested_helper(monkeypatch, tmp_path):
    from sonara.platform.windows import supervisor_loop as sl
    from sonara import paths
    sentinel = tmp_path / "stopped"
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", sentinel)
    assert sl._stop_requested() is False
    sentinel.write_text("")
    assert sl._stop_requested() is True
