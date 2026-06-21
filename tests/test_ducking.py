# tests/test_ducking.py
import json
import sonara.platform.windows.ducking as ducking
from sonara.platform.windows.ducking import AudioDucker, NullDucker


class _FakeVol:
    def __init__(self, v): self.v = v
    def GetMasterVolume(self): return self.v
    def SetMasterVolume(self, v, ctx): self.v = v


class _FakeProc:
    def __init__(self, name): self._n = name
    def name(self): return self._n


class _FakeSession:
    def __init__(self, pid, vol, name="app.exe"):
        self.ProcessId = pid
        self.SimpleAudioVolume = _FakeVol(vol)
        self.Process = _FakeProc(name)


def _sessions(monkeypatch, sessions):
    monkeypatch.setattr(ducking, "_all_sessions", lambda: sessions)


def test_duck_lowers_non_excluded_sessions_to_level(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    s1, s2 = _FakeSession(100, 0.8), _FakeSession(200, 0.6)
    _sessions(monkeypatch, [s1, s2])
    d = AudioDucker()
    d.duck(exclude_pids=set(), level=20)
    assert d.is_ducked() is True
    assert s1.SimpleAudioVolume.v == 0.2     # 20% of full
    assert s2.SimpleAudioVolume.v == 0.2


def test_duck_skips_excluded_pids(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    own, other = _FakeSession(999, 0.9), _FakeSession(100, 0.8)
    _sessions(monkeypatch, [own, other])
    d = AudioDucker()
    d.duck(exclude_pids={999}, level=20)
    assert own.SimpleAudioVolume.v == 0.9    # excluded -> untouched
    assert other.SimpleAudioVolume.v == 0.2


def test_restore_puts_original_volumes_back(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    s = _FakeSession(100, 0.7)
    _sessions(monkeypatch, [s])
    d = AudioDucker()
    d.duck(exclude_pids=set(), level=10)
    assert s.SimpleAudioVolume.v == 0.1
    d.restore()
    assert s.SimpleAudioVolume.v == 0.7
    assert d.is_ducked() is False


def test_duck_is_idempotent(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    s = _FakeSession(100, 0.8)
    _sessions(monkeypatch, [s])
    d = AudioDucker()
    d.duck(set(), 20)
    s.SimpleAudioVolume.v = 0.5               # someone else changed it
    d.duck(set(), 20)                         # second duck must be a no-op
    assert s.SimpleAudioVolume.v == 0.5


def test_duck_writes_state_file_restore_clears_it(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    _sessions(monkeypatch, [_FakeSession(100, 0.8, "vlc.exe")])
    d = AudioDucker()
    d.duck(set(), 20)
    rec = json.loads(state.read_text(encoding="utf-8"))
    assert rec["sessions"][0]["pid"] == 100 and rec["sessions"][0]["original"] == 0.8
    d.restore()
    assert not state.exists()


def test_restore_from_state_file_restores_matching_live_sessions(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    state.write_text(json.dumps({"sessions": [{"pid": 100, "name": "vlc.exe", "original": 0.9}]}),
                     encoding="utf-8")
    live = _FakeSession(100, 0.2, "vlc.exe")   # currently ducked
    _sessions(monkeypatch, [live])
    ducking.restore_from_state_file()
    assert live.SimpleAudioVolume.v == 0.9
    assert not state.exists()


def test_duck_never_raises_on_pycaw_failure(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    def boom(): raise RuntimeError("no COM")
    monkeypatch.setattr(ducking, "_all_sessions", boom)
    d = AudioDucker()
    d.duck(set(), 20)                          # must swallow
    assert d.is_ducked() is False
    d.restore()                                # must swallow


def test_restore_from_state_file_never_raises_on_pycaw_failure(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    import json
    state.write_text(json.dumps({"sessions": [{"pid": 1, "name": "x.exe", "original": 0.5}]}), encoding="utf-8")
    monkeypatch.setattr(ducking, "_all_sessions", lambda: (_ for _ in ()).throw(RuntimeError("no COM")))
    ducking.restore_from_state_file()        # must swallow
    assert not state.exists()                # and still clear the file


def test_null_ducker_is_noop():
    n = NullDucker()
    assert n.is_ducked() is False
    n.duck({1, 2}, 20)
    n.restore()
    assert n.is_ducked() is False


def test_audioducker_methods_are_lock_guarded():
    """AudioDucker must have a threading.Lock that serializes duck/restore/is_ducked."""
    import threading
    d = AudioDucker()
    assert hasattr(d, "_lock"), "AudioDucker must have a _lock attribute"
    assert isinstance(d._lock, type(threading.Lock())), "_lock must be a threading.Lock"
