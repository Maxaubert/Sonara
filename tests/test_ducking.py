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
    assert state.exists()                    # nothing restored -> keep the record (#130)


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


def test_duck_skips_never_duck_audio_engine_processes(monkeypatch, tmp_path):
    # The audio engine / virtual-router (audiodg, SteelSeries Sonar, VoiceMeeter)
    # carries the whole mix; ducking it would lower Sonara's own speech too. It must
    # be skipped by NAME even when not in exclude_pids, while real media still ducks.
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    engine = _FakeSession(500, 0.9, "audiodg.exe")
    router = _FakeSession(501, 0.9, "SteelSeriesSonar.exe")   # case-insensitive
    media = _FakeSession(600, 0.8, "firefox.exe")
    _sessions(monkeypatch, [engine, router, media])
    d = AudioDucker()
    d.duck(exclude_pids=set(), level=20)
    assert engine.SimpleAudioVolume.v == 0.9     # audio engine untouched
    assert router.SimpleAudioVolume.v == 0.9     # virtual router untouched
    assert media.SimpleAudioVolume.v == 0.2      # real media ducked


# ---------------------------------------------------------------------------
# #130: a partial duck or a failed restore must never strand an app ducked
# ---------------------------------------------------------------------------


class _BadVol:
    def GetMasterVolume(self): raise OSError("AUDCLNT_E_DEVICE_INVALIDATED")
    def SetMasterVolume(self, v, ctx): raise OSError("AUDCLNT_E_DEVICE_INVALIDATED")


class _BadSession(_FakeSession):
    def __init__(self, pid, name="bad.exe"):
        super().__init__(pid, 1.0, name)
        self.SimpleAudioVolume = _BadVol()


class _FlakyVol(_FakeVol):
    """SetMasterVolume raises on the Nth call (1-based), succeeds otherwise."""
    def __init__(self, v, fail_on):
        super().__init__(v); self.calls = 0; self.fail_on = fail_on
    def SetMasterVolume(self, v, ctx):
        self.calls += 1
        if self.calls == self.fail_on:
            raise OSError("transient")
        self.v = v


def test_partial_duck_is_recorded_and_restored(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    zen = _FakeSession(100, 1.0, "zen.exe")
    _sessions(monkeypatch, [zen, _BadSession(200)])   # a later session raises
    d = AudioDucker()
    d.duck(set(), 30)
    assert zen.SimpleAudioVolume.v == 0.3
    assert d.is_ducked() is True
    assert [e["name"] for e in json.loads(state.read_text(encoding="utf-8"))["sessions"]] == ["zen.exe"]
    d.restore()
    assert zen.SimpleAudioVolume.v == 1.0
    assert not state.exists()


def test_duck_never_saves_an_already_ducked_level_as_original(monkeypatch, tmp_path):
    monkeypatch.setattr(ducking, "_DUCK_STATE", tmp_path / "duck_state.json")
    zen = _FakeSession(100, 0.3, "zen.exe")            # already at the duck level
    _sessions(monkeypatch, [zen])
    d = AudioDucker()
    d.duck(set(), 30)
    assert d._saved == []                              # 0.3 is never recorded as "original"


def test_failed_restore_retries_by_fresh_enumeration(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    zen = _FakeSession(100, 1.0, "zen.exe")
    zen.SimpleAudioVolume = _FlakyVol(1.0, fail_on=2)  # duck ok, first restore fails
    _sessions(monkeypatch, [zen])
    d = AudioDucker()
    d.duck(set(), 30)
    d.restore()
    assert zen.SimpleAudioVolume.v == 1.0              # retried and restored
    assert not state.exists()
    assert d.is_ducked() is False


def test_unrecoverable_restore_keeps_the_record_and_next_duck_keeps_it(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    zen = _FakeSession(100, 1.0, "zen.exe")
    _sessions(monkeypatch, [zen])
    d = AudioDucker()
    d.duck(set(), 30)
    zen.SimpleAudioVolume = _BadVol()                  # every restore attempt fails
    d.restore()
    assert d.is_ducked() is False
    recs = json.loads(state.read_text(encoding="utf-8"))["sessions"]
    assert recs[0]["name"] == "zen.exe" and recs[0]["original"] == 1.0
    # The device comes back at the ducked level; the next duck skips it (already
    # at target) but must keep the pending record so its real original survives.
    zen.SimpleAudioVolume = _FakeVol(0.3)
    d.duck(set(), 30)
    d.restore()
    assert zen.SimpleAudioVolume.v == 1.0
    assert not state.exists()


def test_restore_from_state_file_keeps_entries_that_failed(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    state.write_text(json.dumps({"sessions": [
        {"pid": 100, "name": "zen.exe", "original": 1.0},
        {"pid": 200, "name": "vlc.exe", "original": 0.8}]}), encoding="utf-8")
    vlc = _FakeSession(200, 0.3, "vlc.exe")
    _sessions(monkeypatch, [_BadSession(100, "zen.exe"), vlc])
    ducking.restore_from_state_file()
    assert vlc.SimpleAudioVolume.v == 0.8
    recs = json.loads(state.read_text(encoding="utf-8"))["sessions"]
    assert [e["name"] for e in recs] == ["zen.exe"]


# --- L-duck-pid: a reused pid must not get another app's volume ------------

def test_crash_restore_ignores_a_reused_pid_of_another_app(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    state.write_text(json.dumps({"sessions": [
        {"pid": 100, "name": "vlc.exe", "original": 0.9}]}), encoding="utf-8")
    stranger = _FakeSession(100, 0.5, "game.exe")    # pid 100 reused
    vlc = _FakeSession(300, 0.2, "vlc.exe")          # vlc restarted on a new pid
    _sessions(monkeypatch, [stranger, vlc])
    ducking.restore_from_state_file()
    assert stranger.SimpleAudioVolume.v == 0.5        # untouched
    assert vlc.SimpleAudioVolume.v == 0.9


def test_crash_restore_still_matches_a_pid_with_no_recorded_name(monkeypatch, tmp_path):
    state = tmp_path / "duck_state.json"
    monkeypatch.setattr(ducking, "_DUCK_STATE", state)
    state.write_text(json.dumps({"sessions": [{"pid": 100, "original": 0.9}]}),
                     encoding="utf-8")
    live = _FakeSession(100, 0.2, "vlc.exe")
    _sessions(monkeypatch, [live])
    ducking.restore_from_state_file()
    assert live.SimpleAudioVolume.v == 0.9
