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


def test_preview_busy_check_is_under_the_daemon_lock(monkeypatch):
    # Two HTTP threads racing preview_voice must coalesce: the check-and-set
    # happens under daemon._lock, so exactly ONE wins.
    daemon, *_ = make_daemon()
    release = threading.Event()

    class H:
        def wait(self, timeout=None):
            release.wait(5)
            return 0
    monkeypatch.setattr(daemon, "_preview_runner", lambda *a, **k: H())
    results = []
    threads = [threading.Thread(target=lambda: results.append(daemon.preview_voice("v")))
               for _ in range(8)]
    for t in threads: t.start()
    for t in threads: t.join()
    release.set()
    assert results.count(True) == 1          # exactly one preview won


def test_preview_flag_resets_when_runner_lookup_raises(monkeypatch):
    daemon, *_ = make_daemon()
    def boom(*a, **k):
        raise RuntimeError("no platform")
    # simulate a raising runner LOOKUP by making the attribute a property-like trap:
    daemon._preview_runner = None
    import sonara.platform as plat
    monkeypatch.setattr(plat, "get_platform", boom)
    assert daemon.preview_voice("v") is False
    assert daemon._preview_busy is False      # not wedged
    # and a later preview still works
    class H:
        def wait(self, timeout=None):
            return 0
    daemon._preview_runner = lambda *a, **k: H()
    assert daemon.preview_voice("v") is True
