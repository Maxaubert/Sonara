import json
import sonara.platform.windows.pausing as pausing
from sonara.platform.windows.pausing import MediaPauser, NullPauser


class _FakeSession:
    def __init__(self, app_id, playing):
        self.app_id = app_id
        self.playing = playing
        self.paused = False
        self.played = False


def _wire(monkeypatch, sessions, tmp_path):
    monkeypatch.setattr(pausing, "_PAUSE_STATE", tmp_path / "pause_state.json")
    monkeypatch.setattr(pausing, "_session_manager", lambda: object())
    monkeypatch.setattr(pausing, "_sessions", lambda mgr: sessions)
    monkeypatch.setattr(pausing, "_is_playing", lambda s: s.playing)
    monkeypatch.setattr(pausing, "_pause", lambda s: setattr(s, "paused", True))
    monkeypatch.setattr(pausing, "_play", lambda s: setattr(s, "played", True))
    monkeypatch.setattr(pausing, "_app_id", lambda s: s.app_id)


def test_pause_pauses_only_playing_sessions(monkeypatch, tmp_path):
    playing, stopped = _FakeSession("spotify", True), _FakeSession("game", False)
    _wire(monkeypatch, [playing, stopped], tmp_path)
    p = MediaPauser()
    p.pause()
    assert p.is_paused() is True
    assert playing.paused is True
    assert stopped.paused is False


def test_resume_plays_only_previously_paused(monkeypatch, tmp_path):
    playing, stopped = _FakeSession("spotify", True), _FakeSession("game", False)
    _wire(monkeypatch, [playing, stopped], tmp_path)
    p = MediaPauser()
    p.pause()
    p.resume()
    assert p.is_paused() is False
    assert playing.played is True
    assert stopped.played is False


def test_resume_skips_a_session_that_vanished(monkeypatch, tmp_path):
    a, b = _FakeSession("a", True), _FakeSession("b", True)
    _wire(monkeypatch, [a, b], tmp_path)
    p = MediaPauser()
    p.pause()
    monkeypatch.setattr(pausing, "_sessions", lambda mgr: [b])  # 'a' is gone
    p.resume()
    assert b.played is True                       # still resumes the survivor


def test_pause_writes_state_resume_clears_it(monkeypatch, tmp_path):
    state = tmp_path / "pause_state.json"
    s = _FakeSession("spotify", True)
    _wire(monkeypatch, [s], tmp_path)
    p = MediaPauser()
    p.pause()
    assert json.loads(state.read_text())["apps"] == ["spotify"]
    p.resume()
    assert not state.exists()


def test_pause_never_raises_on_backend_failure(monkeypatch, tmp_path):
    monkeypatch.setattr(pausing, "_PAUSE_STATE", tmp_path / "pause_state.json")

    def boom():
        raise RuntimeError("winrt down")

    monkeypatch.setattr(pausing, "_session_manager", boom)
    p = MediaPauser()
    p.pause()                                     # must not raise
    assert p.is_paused() is False


def test_resume_from_state_file_plays_recorded_and_deletes(monkeypatch, tmp_path):
    state = tmp_path / "pause_state.json"
    state.write_text(json.dumps({"apps": ["spotify"]}), encoding="utf-8")
    survivor = _FakeSession("spotify", False)
    monkeypatch.setattr(pausing, "_PAUSE_STATE", state)
    monkeypatch.setattr(pausing, "_session_manager", lambda: object())
    monkeypatch.setattr(pausing, "_sessions", lambda mgr: [survivor])
    monkeypatch.setattr(pausing, "_play", lambda s: setattr(s, "played", True))
    monkeypatch.setattr(pausing, "_app_id", lambda s: s.app_id)
    pausing.resume_from_state_file()
    assert survivor.played is True
    assert not state.exists()


def test_null_pauser_is_noop():
    n = NullPauser()
    assert n.is_paused() is False
    n.pause(); n.resume()                         # no error, no state
    assert n.is_paused() is False
