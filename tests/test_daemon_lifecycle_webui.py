from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def test_shutdown_stay_down_writes_sentinel(monkeypatch, tmp_path):
    from sonara import paths
    sentinel = tmp_path / "stopped"
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", sentinel)
    daemon, *_ = make_daemon()
    stopped = []
    monkeypatch.setattr(daemon, "stop", lambda: stopped.append(True))
    import threading
    real_timer = threading.Timer
    monkeypatch.setattr(threading, "Timer",
                        lambda d, fn: real_timer(0.01, fn))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SHUTDOWN,
                           "stay_down": True})
    import time
    time.sleep(0.3)
    assert sentinel.exists()
    assert stopped


def test_plain_shutdown_leaves_no_sentinel(monkeypatch, tmp_path):
    from sonara import paths
    sentinel = tmp_path / "stopped"
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", sentinel)
    daemon, *_ = make_daemon()
    monkeypatch.setattr(daemon, "stop", lambda: None)
    import threading
    real_timer = threading.Timer
    monkeypatch.setattr(threading, "Timer", lambda d, fn: real_timer(0.01, fn))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SHUTDOWN})
    import time
    time.sleep(0.3)
    assert not sentinel.exists()
