"""The daemon builds missing preview files in a background thread (#38)."""
import threading

from tests.daemon_helpers import make_daemon


def test_preview_builder_runs_ensure_all_off_thread(monkeypatch):
    daemon, *_ = make_daemon()
    done = threading.Event()
    calls = []

    def fake_ensure(voices, **kw):
        calls.append(voices)
        done.set()
        return 0
    import sonara.previews as previews
    import sonara.webui as webui
    monkeypatch.setattr(previews, "ensure_all", fake_ensure)
    monkeypatch.setattr(webui, "_installed_voices",
                        lambda: {"kokoro": ["af_heart"]})
    daemon._start_preview_builder(delay_s=0)
    assert done.wait(5)
    assert calls == [{"kokoro": ["af_heart"]}]


def test_preview_builder_failure_is_contained(monkeypatch):
    daemon, *_ = make_daemon()
    import sonara.webui as webui
    def boom():
        raise RuntimeError("no engines")
    monkeypatch.setattr(webui, "_installed_voices", boom)
    t = daemon._start_preview_builder(delay_s=0)
    t.join(5)
    assert not t.is_alive()                      # died quietly, took nothing down
