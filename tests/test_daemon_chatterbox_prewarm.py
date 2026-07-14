"""Daemon pre-warms the chatterbox worker at startup / on voice switch so the
first digest does not pay the ~40s cold model load. Only when the selected voice
is a chatterbox voice and chatterbox is provisioned."""
from tests.daemon_helpers import make_daemon


def _stub_chatterbox(monkeypatch, *, provisioned=True, is_cb=True):
    import sonara.chatterbox as cb
    warmed = []
    monkeypatch.setattr(cb, "is_provisioned", lambda: provisioned)
    monkeypatch.setattr(cb, "is_chatterbox_voice", lambda v: is_cb)
    monkeypatch.setattr(cb.CLIENT, "warm", lambda cfg: warmed.append(cfg) or True)
    return warmed


def _run_warm(daemon):
    # call the warm hook synchronously by capturing the thread target
    import threading
    started = {}
    orig = threading.Thread

    def capture(*a, **k):
        if k.get("name") == "sonara-cb-warm":
            k["target"]()                     # run inline for the test
            started["ran"] = True
            class _Noop:
                def start(self_): pass
            return _Noop()
        return orig(*a, **k)
    return capture, started


def test_prewarm_runs_for_chatterbox_voice(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["voice"] = "shadowheart"
    warmed = _stub_chatterbox(monkeypatch)
    cap, started = _run_warm(daemon)
    monkeypatch.setattr("threading.Thread", cap)
    daemon._maybe_prewarm_chatterbox()
    assert started.get("ran") and len(warmed) == 1


def test_no_prewarm_for_kokoro_voice(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["voice"] = "af_heart"
    warmed = _stub_chatterbox(monkeypatch, is_cb=False)
    daemon._maybe_prewarm_chatterbox()
    assert warmed == []


def test_no_prewarm_when_not_provisioned(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["voice"] = "shadowheart"
    warmed = _stub_chatterbox(monkeypatch, provisioned=False)
    daemon._maybe_prewarm_chatterbox()
    assert warmed == []


