"""CLI lifecycle commands (#23): sonara shutdown / sonara start, and the
stop-before-mutate ordering in install/uninstall."""
import os

import pytest
from sonara import cli, paths
from sonara.protocol import MsgType


class FakeSup:
    def __init__(self):
        self.ended = 0

    def end_task(self):
        self.ended += 1


def _wire(monkeypatch, tmp_path, connectable=False):
    """Common lifecycle test wiring: tmp sentinel, no-op sleeps, recorded sends."""
    sentinel = tmp_path / "stopped"
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", sentinel)
    monkeypatch.setattr(paths, "ensure_sonara_dir", lambda: None)
    monkeypatch.setattr(paths, "socket_connectable", lambda: connectable)
    monkeypatch.setattr(cli.time, "sleep", lambda s: None)
    sent = []
    monkeypatch.setattr(cli, "_send",
                        lambda msg, expect_reply=False: sent.append(msg) or {"ok": True})
    return sentinel, sent


def test_stop_sonara_writes_sentinel_ends_task_sends_shutdown(monkeypatch, tmp_path):
    sentinel, sent = _wire(monkeypatch, tmp_path, connectable=False)
    sup = FakeSup()
    assert cli.stop_sonara(sup) is True
    assert sentinel.exists()                          # respawn paths gated
    assert sup.ended == 1                             # scheduled task ended
    assert any(m.get("type") == MsgType.SHUTDOWN for m in sent)


def test_stop_sonara_tolerates_daemon_not_running(monkeypatch, tmp_path):
    sentinel, _ = _wire(monkeypatch, tmp_path, connectable=False)

    def boom(msg, expect_reply=False):
        raise OSError("daemon not running")
    monkeypatch.setattr(cli, "_send", boom)
    assert cli.stop_sonara(FakeSup()) is True         # not running == stopped


def test_stop_sonara_reports_failure_when_daemon_stays_up(monkeypatch, tmp_path):
    _wire(monkeypatch, tmp_path, connectable=True)    # socket never goes away
    monkeypatch.setattr(cli.time, "time",
                        _ticker(step=2.0))            # deadline expires fast
    assert cli.stop_sonara(FakeSup()) is False


def _ticker(step):
    state = {"t": 0.0}

    def fake_time():
        state["t"] += step
        return state["t"]
    return fake_time


def test_start_sonara_clears_sentinel_and_spawns(monkeypatch, tmp_path):
    sentinel, _ = _wire(monkeypatch, tmp_path, connectable=True)
    sentinel.write_text("")
    spawned = []
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "ensure_running",
                        lambda: spawned.append(True))
    rc = cli.start_sonara()
    assert rc == 0
    assert not sentinel.exists()                      # start clears the gate
    assert spawned


def test_shutdown_and_start_subcommands_registered():
    parser = cli._build_parser()
    args = parser.parse_args(["shutdown"])
    assert args.func is cli._cmd_shutdown
    args = parser.parse_args(["start"])
    assert args.func is cli._cmd_start


def test_speech_stop_subcommand_unchanged():
    # `sonara stop` keeps its existing meaning: stop SPEECH, not the daemon.
    parser = cli._build_parser()
    args = parser.parse_args(["stop"])
    assert args.func is not getattr(cli, "_cmd_shutdown", None)
