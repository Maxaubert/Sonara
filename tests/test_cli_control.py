import json
from unittest import mock

import pytest

from echo import cli
from echo.protocol import MsgType, PROTOCOL_VERSION


def _sent(send_mock):
    assert send_mock.call_count == 1, send_mock.call_args_list
    args, kwargs = send_mock.call_args
    return args[0], args, kwargs


def test_status_sends_status_and_prints(capsys):
    reply = {"verbosity": "everything", "rate": 200, "voice": None,
             "foreground": "abc", "queue_len": 3}
    with mock.patch("echo.client.send", return_value=reply) as send:
        rc = cli.main(["status"])
    msg, args, kwargs = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.STATUS}
    assert kwargs.get("expect_reply") is True
    out = capsys.readouterr().out
    assert "everything" in out
    assert "queue_len" in out or "queue" in out


def test_status_handles_no_reply(capsys):
    with mock.patch("echo.client.send", return_value=None):
        rc = cli.main(["status"])
    assert rc == 1
    assert "no response" in capsys.readouterr().out.lower()


def test_verbosity_sends_set_verbosity():
    with mock.patch("echo.client.send", return_value=None) as send:
        rc = cli.main(["verbosity", "quiet"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_VERBOSITY,
                   "verbosity": "quiet"}


def test_verbosity_rejects_bad_value():
    with mock.patch("echo.client.send") as send:
        with pytest.raises(SystemExit):
            cli.main(["verbosity", "loud"])
    send.assert_not_called()


def test_rate_sends_int_set_rate():
    with mock.patch("echo.client.send", return_value=None) as send:
        rc = cli.main(["rate", "260"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_RATE, "rate": 260}
    assert isinstance(msg["rate"], int)


def test_voice_sends_set_voice():
    with mock.patch("echo.client.send", return_value=None) as send:
        rc = cli.main(["voice", "Ava (Premium)"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_VOICE,
                   "voice": "Ava (Premium)"}


def test_repeat_sends_repeat():
    with mock.patch("echo.client.send", return_value=None) as send:
        cli.main(["repeat"])
    msg, _, _ = _sent(send)
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.REPEAT}


def test_stop_sends_stop():
    with mock.patch("echo.client.send", return_value=None) as send:
        cli.main(["stop"])
    msg, _, _ = _sent(send)
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.STOP}


def test_skip_sends_skip():
    with mock.patch("echo.client.send", return_value=None) as send:
        cli.main(["skip"])
    msg, _, _ = _sent(send)
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SKIP}


def test_no_args_prints_help_and_returns_2(capsys):
    rc = cli.main([])
    assert rc == 2
    err = capsys.readouterr()
    assert "usage" in (err.out + err.err).lower()


def test_rate_rejects_non_integer_wpm():
    with mock.patch("echo.client.send") as send:
        with pytest.raises(SystemExit):
            cli.main(["rate", "fast"])
    send.assert_not_called()
