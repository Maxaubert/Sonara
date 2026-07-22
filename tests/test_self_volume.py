"""Instant self-session volume: the attenuation half of speech volume."""
import os

from sonara.platform.windows import self_volume


class FakeVol:
    def __init__(self):
        self.set = []

    def SetMasterVolume(self, scalar, ctx):
        self.set.append(scalar)


class FakeSession:
    def __init__(self, pid, vol=None):
        self.ProcessId = pid
        self.SimpleAudioVolume = vol


def test_sets_only_own_process_session(monkeypatch):
    mine, other = FakeVol(), FakeVol()
    sessions = [FakeSession(os.getpid(), mine), FakeSession(4242, other)]
    monkeypatch.setattr(self_volume, "_sessions", lambda: sessions)
    assert self_volume.apply_self_volume(50) is True
    assert mine.set == [0.5]
    assert other.set == []


def test_clamps_above_100_to_unity(monkeypatch):
    mine = FakeVol()
    monkeypatch.setattr(self_volume, "_sessions",
                        lambda: [FakeSession(os.getpid(), mine)])
    assert self_volume.apply_self_volume(150) is True
    assert mine.set == [1.0]


def test_no_own_session_returns_false(monkeypatch):
    monkeypatch.setattr(self_volume, "_sessions",
                        lambda: [FakeSession(4242, FakeVol())])
    assert self_volume.apply_self_volume(50) is False


def test_com_failure_returns_false(monkeypatch):
    def boom():
        raise RuntimeError("no COM here")
    monkeypatch.setattr(self_volume, "_sessions", boom)
    assert self_volume.apply_self_volume(50) is False
