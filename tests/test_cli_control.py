from unittest import mock

import pytest

from sonara import cli
from sonara.protocol import MsgType, PROTOCOL_VERSION


def _sent(send_mock):
    assert send_mock.call_count == 1, send_mock.call_args_list
    args, kwargs = send_mock.call_args
    return args[0], args, kwargs


def test_status_sends_status_and_prints(capsys):
    reply = {"verbosity": "everything", "rate": 200, "voice": None,
             "foreground": "abc", "queue_len": 3}
    with mock.patch("sonara.client.send", return_value=reply) as send:
        rc = cli.main(["status"])
    msg, args, kwargs = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.STATUS}
    assert kwargs.get("expect_reply") is True
    out = capsys.readouterr().out
    assert "everything" in out
    assert "queue_len" in out or "queue" in out


def test_status_handles_no_reply(capsys):
    with mock.patch("sonara.client.send", return_value=None):
        rc = cli.main(["status"])
    assert rc == 1
    assert "no response" in capsys.readouterr().out.lower()


def test_verbosity_sends_set_verbosity():
    with mock.patch("sonara.client.send", return_value=None) as send:
        rc = cli.main(["verbosity", "quiet"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_VERBOSITY,
                   "verbosity": "quiet"}


def test_verbosity_rejects_bad_value():
    with mock.patch("sonara.client.send") as send:
        with pytest.raises(SystemExit):
            cli.main(["verbosity", "loud"])
    send.assert_not_called()


def test_rate_sends_int_set_rate():
    with mock.patch("sonara.client.send", return_value=None) as send:
        rc = cli.main(["rate", "260"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_RATE, "rate": 260}
    assert isinstance(msg["rate"], int)


def test_minqueue_sends_int_set_minqueue():
    with mock.patch("sonara.client.send", return_value=None) as send:
        rc = cli.main(["minqueue", "3"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_MINQUEUE,
                   "minqueue": 3}
    assert isinstance(msg["minqueue"], int)


def test_minqueue_rejects_non_integer():
    with mock.patch("sonara.client.send") as send:
        with pytest.raises(SystemExit):
            cli.main(["minqueue", "lots"])
    send.assert_not_called()


def test_voice_sends_set_voice():
    with mock.patch("sonara.client.send", return_value=None) as send:
        rc = cli.main(["voice", "Ava (Premium)"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_VOICE,
                   "voice": "Ava (Premium)"}


def test_voice_joins_multiword_name_without_quotes():
    # "Microsoft David" arrives as two argv tokens; the CLI joins them so the user
    # doesn't have to quote multi-word voice names.
    with mock.patch("sonara.client.send", return_value=None) as send:
        rc = cli.main(["voice", "Microsoft", "David"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_VOICE,
                   "voice": "Microsoft David"}


def test_voice_no_arg_lists_voices_without_changing_anything(capsys):
    class _V:
        def __init__(self, n):
            self.display_name = n

    fake = mock.Mock()
    fake.tts.list_voices.return_value = [_V("Microsoft David"), _V("Microsoft Zira")]
    with mock.patch("sonara.cli._platform", return_value=fake), \
            mock.patch("sonara.client.send", return_value=None) as send:
        rc = cli.main(["voice"])
    out = capsys.readouterr().out
    assert rc == 0
    assert "Microsoft David" in out and "Microsoft Zira" in out
    send.assert_not_called()   # listing must not set the voice


def test_repeat_sends_repeat():
    with mock.patch("sonara.client.send", return_value=None) as send:
        cli.main(["repeat"])
    msg, _, _ = _sent(send)
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.REPEAT}


def test_stop_sends_stop():
    with mock.patch("sonara.client.send", return_value=None) as send:
        cli.main(["stop"])
    msg, _, _ = _sent(send)
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.STOP}


def test_skip_sends_skip():
    with mock.patch("sonara.client.send", return_value=None) as send:
        cli.main(["skip"])
    msg, _, _ = _sent(send)
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SKIP}


def test_no_args_prints_help_and_returns_2(capsys):
    rc = cli.main([])
    assert rc == 2
    err = capsys.readouterr()
    assert "usage" in (err.out + err.err).lower()


def test_rate_rejects_non_integer_wpm():
    with mock.patch("sonara.client.send") as send:
        with pytest.raises(SystemExit):
            cli.main(["rate", "fast"])
    send.assert_not_called()


# ---------------------------------------------------------------------------
# Daemon-down: friendly message, non-zero exit, no traceback
# ---------------------------------------------------------------------------

CONTROL_SUBCOMMANDS = [
    ["status"],
    ["verbosity", "quiet"],
    ["rate", "200"],
    ["voice", "Samantha"],
    ["repeat"],
    ["stop"],
    ["skip"],
]


@pytest.mark.parametrize("argv", CONTROL_SUBCOMMANDS)
def test_daemon_down_prints_friendly_message_and_exits_nonzero(argv, capsys):
    """When the daemon is down all control subcommands must print a friendly
    message to stderr and return non-zero — no raw traceback."""
    from sonara.client import DaemonNotRunning

    with mock.patch("sonara.client.send", side_effect=DaemonNotRunning(
        "Sonara daemon is not running. Run: sonara install"
    )):
        rc = cli.main(argv)

    assert rc != 0, f"Expected non-zero exit for {argv}"
    captured = capsys.readouterr()
    combined = captured.out + captured.err
    # Must contain a human-readable hint
    assert "sonara install" in combined.lower() or "not running" in combined.lower(), (
        f"No friendly message found in output: {combined!r}"
    )
    # Must NOT contain a raw traceback
    assert "Traceback" not in combined, (
        f"Raw traceback leaked into output for {argv}: {combined!r}"
    )


def test_daemon_down_message_goes_to_stderr(capsys):
    """The friendly daemon-down message must go to stderr, not stdout."""
    from sonara.client import DaemonNotRunning

    with mock.patch("sonara.client.send", side_effect=DaemonNotRunning(
        "Sonara daemon is not running. Run: sonara install"
    )):
        rc = cli.main(["stop"])

    assert rc != 0
    captured = capsys.readouterr()
    assert "not running" in captured.err.lower() or "sonara install" in captured.err.lower()


def test_client_send_raises_daemon_not_running_on_connection_refused(tmp_path, monkeypatch):
    """send() must raise DaemonNotRunning (not a raw OSError) when the
    lockfile is absent, making the error cleanly catchable."""
    import sonara.client as client_mod
    from sonara.client import DaemonNotRunning

    missing_lock = tmp_path / "daemon.lock"  # nonexistent -> transport.connect raises
    monkeypatch.setattr(client_mod, "LOCK_PATH", missing_lock, raising=False)

    with pytest.raises(DaemonNotRunning):
        client_mod.send({"type": "ping"})


def test_rate_subcommand_sends_absolute_set_rate():
    with mock.patch("sonara.client.send", return_value=None) as send:
        rc = cli.main(["rate", "300"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_RATE, "rate": 300}
    assert "delta" not in msg  # absolute, not a delta


def test_voice_subcommand_sends_set_voice():
    with mock.patch("sonara.client.send", return_value=None) as send:
        rc = cli.main(["voice", "Zoe (Premium)"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SET_VOICE,
                   "voice": "Zoe (Premium)"}


def test_skip_subcommand_sends_skip():
    with mock.patch("sonara.client.send", return_value=None) as send:
        rc = cli.main(["skip"])
    msg, _, _ = _sent(send)
    assert rc == 0
    assert msg == {"v": PROTOCOL_VERSION, "type": MsgType.SKIP}


def test_summary_on_sends_set_summary_mode():
    from unittest import mock
    from sonara import cli
    sent = []
    with mock.patch("sonara.client.send", side_effect=lambda m, **k: sent.append(m)):
        rc = cli.main(["summary", "on"])
    assert rc == 0
    assert sent[-1]["type"] == "set_summary_mode" and sent[-1]["enabled"] is True


def test_summary_off_sends_disabled():
    from unittest import mock
    from sonara import cli
    sent = []
    with mock.patch("sonara.client.send", side_effect=lambda m, **k: sent.append(m)):
        rc = cli.main(["summary", "off"])
    assert rc == 0
    assert sent[-1]["enabled"] is False


def test_bare_summary_prints_state(capsys, monkeypatch, tmp_path):
    from sonara import cli, config
    monkeypatch.setattr(config, "CONFIG_PATH", tmp_path / "config.json")
    rc = cli.main(["summary"])
    assert rc == 0
    assert "off" in capsys.readouterr().out.lower()      # default state
