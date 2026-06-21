import sonara.cli as cli
from sonara.protocol import MsgType


def test_audio_control_on_sends_enabled_true(monkeypatch):
    sent = {}
    monkeypatch.setattr(cli, "_send", lambda m, expect_reply=False: sent.update(m))
    assert cli.main(["audio-control", "on"]) == 0
    assert sent["type"] == MsgType.SET_AUDIO_CONTROL and sent["enabled"] is True


def test_audio_control_off_sends_enabled_false(monkeypatch):
    sent = {}
    monkeypatch.setattr(cli, "_send", lambda m, expect_reply=False: sent.update(m))
    assert cli.main(["audio-control", "off"]) == 0
    assert sent["enabled"] is False


def test_duck_level_forwards_integer(monkeypatch):
    sent = {}
    monkeypatch.setattr(cli, "_send", lambda m, expect_reply=False: sent.update(m))
    assert cli.main(["duck-level", "35"]) == 0
    assert sent["type"] == MsgType.SET_DUCK_LEVEL and sent["level"] == 35
