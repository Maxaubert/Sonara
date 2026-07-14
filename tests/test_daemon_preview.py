import threading

from tests.daemon_helpers import make_daemon


def test_preview_voice_speaks_sample_with_named_voice(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon()
    ran = []
    done = threading.Event()

    class H:
        def wait(self, timeout=None):
            done.set()
            return 0
    def fake_run(text, voice, rate, on_play=None):
        ran.append((text, voice, rate))
        return H()
    monkeypatch.setattr(daemon, "_preview_runner", fake_run)
    assert daemon.preview_voice("af_bella") is True
    assert done.wait(2)
    text, voice, rate = ran[0]
    assert voice == "af_bella"
    assert "af_bella" in text
    assert daemon.config["voice"] != "af_bella"       # config untouched


def test_preview_voice_coalesces(monkeypatch):
    daemon, *_ = make_daemon()
    release = threading.Event()

    class H:
        def wait(self, timeout=None):
            release.wait(5)
            return 0
    monkeypatch.setattr(daemon, "_preview_runner", lambda *a, **k: H())
    assert daemon.preview_voice("af_bella") is True
    assert daemon.preview_voice("af_heart") is False   # busy
    release.set()
