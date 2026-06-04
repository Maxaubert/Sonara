from unittest import mock

from echo.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


def test_set_rate_updates_config_and_speaker_and_saves():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("echo.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_RATE, rate=150))
    assert config["rate"] == 150
    assert speaker.rates == [150]
    save.assert_called_once_with(config)


def test_set_voice_updates_config_and_speaker_and_saves():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("echo.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_VOICE, voice="Ava (Premium)"))
    assert config["voice"] == "Ava (Premium)"
    assert speaker.voices == ["Ava (Premium)"]
    save.assert_called_once_with(config)


def test_set_verbosity_updates_config_and_saves_no_speaker_call():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("echo.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_VERBOSITY, verbosity="quiet"))
    assert config["verbosity"] == "quiet"
    assert speaker.rates == []
    assert speaker.voices == []
    save.assert_called_once_with(config)


def test_status_returns_documented_dict():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    config["rate"] = 175
    config["voice"] = "Samantha"
    # enqueue two items so queue_len is reported
    from echo.queue import SpeechItem
    queue.enqueue(SpeechItem(id=1, session="fg", kind="prose", text="a", is_decision=False))
    queue.enqueue(SpeechItem(id=2, session="fg", kind="prose", text="b", is_decision=False))
    resp = daemon.handle_message(_msg(MsgType.STATUS))
    assert resp == {
        "verbosity": "medium",
        "rate": 175,
        "voice": "Samantha",
        "foreground": "fg",
        "queue_len": 2,
    }


def test_ping_returns_ok():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    resp = daemon.handle_message(_msg(MsgType.PING))
    assert resp == {"ok": True}


def test_unknown_type_returns_none():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon.handle_message(_msg("totally_unknown")) is None
