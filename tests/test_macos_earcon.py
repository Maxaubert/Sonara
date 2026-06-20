from sonara.platform.macos import earcon as mod
from sonara.platform.macos.earcon import MacEarconBackend


def test_play_invokes_afplay_with_path(monkeypatch):
    seen = {}
    monkeypatch.setattr(mod.os.path, "exists", lambda p: True)
    monkeypatch.setattr(mod.subprocess, "Popen", lambda args: seen.setdefault("args", args))
    MacEarconBackend().play("/x/Funk.aiff")
    assert seen["args"] == ["afplay", "/x/Funk.aiff"]


def test_play_missing_file_is_none(monkeypatch):
    monkeypatch.setattr(mod.os.path, "exists", lambda p: False)
    assert MacEarconBackend().play("/nope.aiff") is None


def test_default_earcons_are_macos_system_sounds():
    d = MacEarconBackend().default_earcons()
    assert d["permission"] == "/System/Library/Sounds/Funk.aiff"
    assert set(d) == {"permission", "choice", "plan", "error", "turn_done", "ready"}
