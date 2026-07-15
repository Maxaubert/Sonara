from unittest import mock

from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


def test_set_rate_updates_config_and_speaker_and_saves():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("sonara.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_RATE, rate=150))
    assert config["rate"] == 150
    assert speaker.rates == [150]
    save.assert_called_once_with(config)


def test_set_rate_absolute_rejects_non_numeric():
    # Regression #6: an absolute rate that isn't an int must NOT be stored -- it
    # would poison config (persisted to disk) and break synthesis on every
    # utterance, silently muting the daemon until the bad config is removed.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    before = config["rate"]
    with mock.patch("sonara.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_RATE, rate="abc"))
    assert config["rate"] == before
    assert speaker.rates == []
    save.assert_not_called()


def test_set_rate_absolute_clamps_out_of_range():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("sonara.daemon.save_config"):
        daemon.handle_message(_msg(MsgType.SET_RATE, rate=999999))
    assert config["rate"] == 400           # clamped to RATE_MAX
    with mock.patch("sonara.daemon.save_config"):
        daemon.handle_message(_msg(MsgType.SET_RATE, rate=1))
    assert config["rate"] == 100           # clamped to RATE_MIN


def test_set_voice_updates_config_and_speaker_and_saves():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("sonara.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_VOICE, voice="Ava (Premium)"))
    assert config["voice"] == "Ava (Premium)"
    assert speaker.voices == ["Ava (Premium)"]
    save.assert_called_once_with(config)


def test_set_verbosity_updates_config_and_saves_no_speaker_call():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("sonara.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_VERBOSITY, verbosity="quiet"))
    assert config["verbosity"] == "quiet"
    assert speaker.rates == []
    assert speaker.voices == []
    save.assert_called_once_with(config)


def test_set_minqueue_updates_config_and_saves():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("sonara.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_MINQUEUE, minqueue=3))
    assert config["minqueue"] == 3
    save.assert_called_once_with(config)


def test_set_minqueue_clamps_out_of_range():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    with mock.patch("sonara.daemon.save_config"):
        daemon.handle_message(_msg(MsgType.SET_MINQUEUE, minqueue=999))
    assert config["minqueue"] == 10          # clamped to MINQUEUE_MAX
    with mock.patch("sonara.daemon.save_config"):
        daemon.handle_message(_msg(MsgType.SET_MINQUEUE, minqueue=0))
    assert config["minqueue"] == 0           # 0 = start immediately (valid floor)
    with mock.patch("sonara.daemon.save_config"):
        daemon.handle_message(_msg(MsgType.SET_MINQUEUE, minqueue=-3))
    assert config["minqueue"] == 0           # clamped to MINQUEUE_MIN


def test_set_minqueue_rejects_non_numeric():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    before = config["minqueue"]
    with mock.patch("sonara.daemon.save_config") as save:
        daemon.handle_message(_msg(MsgType.SET_MINQUEUE, minqueue="abc"))
    assert config["minqueue"] == before
    save.assert_not_called()


def test_status_returns_documented_dict():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    config["rate"] = 175
    config["voice"] = "Samantha"
    config["minqueue"] = 4
    resp = daemon.handle_message(_msg(MsgType.STATUS))
    assert resp == {
        "verbosity": "medium",
        "rate": 175,
        "voice": "Samantha",
        "foreground": "fg",
        "minqueue": 4,
        "summary_mode": False,
    }


def test_ping_returns_ok():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    resp = daemon.handle_message(_msg(MsgType.PING))
    assert resp == {"ok": True}


def test_unknown_type_returns_none():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon.handle_message(_msg("totally_unknown")) is None
