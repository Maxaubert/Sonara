import pytest

import sonara.cli as cli
from sonara.protocol import MsgType


def test_audio_control_verb_is_gone(monkeypatch, capsys):
    # The pre-#92 audio-control shim was removed; audio-mode replaces it.
    sent = []
    monkeypatch.setattr(cli, "_send", lambda m, expect_reply=False: sent.append(m))
    with pytest.raises(SystemExit):
        cli.main(["audio-control", "on"])
    assert sent == []


def test_duck_level_forwards_integer(monkeypatch):
    sent = {}
    monkeypatch.setattr(cli, "_send", lambda m, expect_reply=False: sent.update(m))
    assert cli.main(["duck-level", "35"]) == 0
    assert sent["type"] == MsgType.SET_DUCK_LEVEL and sent["level"] == 35


def test_audio_mode_command_sends_set_audio_mode(monkeypatch):
    sent = {}
    monkeypatch.setattr(cli, "_send", lambda m, expect_reply=False: sent.update(m))
    assert cli.main(["audio-mode", "pause"]) == 0
    assert sent["type"] == MsgType.SET_AUDIO_MODE and sent["mode"] == "pause"
