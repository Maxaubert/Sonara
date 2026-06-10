from sonari.platform.macos import tts as mod
from sonari.platform.macos.tts import MacTtsBackend


def test_run_builds_say_command_with_voice_and_rate(monkeypatch):
    calls = {}
    class _P:  # fake Popen
        def __init__(self, cmd): calls["cmd"] = cmd
    monkeypatch.setattr(mod.subprocess, "Popen", _P)
    MacTtsBackend().run("Hi", "Ava", 220)
    assert calls["cmd"] == ["say", "-v", "Ava", "-r", "220", "Hi"]


def test_best_voice_prefers_premium_en(monkeypatch):
    listing = "Ava (Premium)   en_US  # hi\nDaniel          en_GB  # hi\n"
    monkeypatch.setattr(mod.subprocess, "check_output", lambda *a, **k: listing)
    assert MacTtsBackend().best_voice() == "Ava"


def test_best_voice_falls_back_when_say_errors(monkeypatch):
    def boom(*a, **k): raise FileNotFoundError()
    monkeypatch.setattr(mod.subprocess, "check_output", boom)
    assert MacTtsBackend().best_voice() == "Samantha"
