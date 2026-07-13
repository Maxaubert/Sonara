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


# --- install stop-then-swap + uninstall ordering (#23) --------------------

def test_copy_app_failure_leaves_live_app_intact(monkeypatch, tmp_path):
    # The old rmtree-then-copytree gutted a locked APP_DIR (the documented
    # 'gutted app' failure): a failed copy must leave the live app untouched.
    app = tmp_path / "app"
    live = app / "sonara"
    live.mkdir(parents=True)
    (live / "daemon.py").write_text("LIVE")
    monkeypatch.setattr(paths, "APP_DIR", app)

    def boom(src, dst):
        raise OSError("disk full")
    monkeypatch.setattr(cli.shutil, "copytree", boom)
    with pytest.raises(OSError):
        cli._copy_app(str(tmp_path / "plugin"))
    assert (live / "daemon.py").read_text() == "LIVE"     # untouched


def test_copy_app_swaps_and_cleans_residue(monkeypatch, tmp_path):
    app = tmp_path / "app"
    live = app / "sonara"
    live.mkdir(parents=True)
    (live / "daemon.py").write_text("OLD")
    (app / "sonara.old").mkdir()                          # stale prior-crash residue
    (app / "sonara.new").mkdir()
    plugin = tmp_path / "plugin"
    srcpkg = plugin / "src" / "sonara"
    srcpkg.mkdir(parents=True)
    (srcpkg / "daemon.py").write_text("NEW")
    monkeypatch.setattr(paths, "APP_DIR", app)
    out = cli._copy_app(str(plugin))
    assert out == str(app)
    assert (live / "daemon.py").read_text() == "NEW"      # swapped in
    assert not (app / "sonara.new").exists()
    assert not (app / "sonara.old").exists()              # residue cleaned


def test_install_stops_before_copying_and_clears_sentinel(monkeypatch, tmp_path):
    from tests._fakeplatform import fake_platform, FakeSupervisor, FakeHotkey, FakeTts
    from sonara import kokoro_provision as kp
    sentinel = tmp_path / "stopped"
    sentinel.write_text("")                               # previously shut down
    monkeypatch.setattr(paths, "STOPPED_SENTINEL_PATH", sentinel)
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)
    monkeypatch.setattr(cli, "_winrt_importable", lambda python: True)
    order = []
    sup = FakeSupervisor(python="/PY/pythonw.exe")
    pb = fake_platform(supervisor=sup, hotkey=FakeHotkey(ok=True, detail="ok"),
                       tts=FakeTts("Aria"))
    monkeypatch.setattr(cli, "_platform", lambda: pb)
    monkeypatch.setattr(cli, "stop_sonara",
                        lambda s=None: order.append("stop") or True)
    monkeypatch.setattr(cli, "_copy_app",
                        lambda root: order.append("copy") or str(tmp_path / "app"))
    monkeypatch.setattr(cli, "_write_install_record", lambda **k: None)
    monkeypatch.setattr(cli, "_read_plugin_version", lambda root: "0.5.0")
    monkeypatch.setattr("sonara.keymap.migrate_default_chord", lambda: None)
    monkeypatch.setattr("sonara.keymap.write_default_keymap_if_absent", lambda: None)
    monkeypatch.setattr("sonara.keymap.write_resolved", lambda: None)
    monkeypatch.setattr("sonara.paths.ensure_sonara_dir", lambda: None)
    rc = cli.install()
    assert rc == 0
    assert order[:2] == ["stop", "copy"]                  # stop BEFORE the copy
    assert not sentinel.exists()                          # install leaves it startable


def test_uninstall_stops_before_removing(monkeypatch, tmp_path):
    from tests._fakeplatform import fake_platform, FakeSupervisor
    order = []
    sup = FakeSupervisor()
    sup.uninstall = lambda: order.append("rm-task")
    monkeypatch.setattr(cli, "_platform",
                        lambda: fake_platform(supervisor=sup))
    monkeypatch.setattr(cli, "stop_sonara",
                        lambda s=None: order.append("stop") or True)
    rc = cli.uninstall()
    assert rc == 0
    assert order.index("stop") < order.index("rm-task")   # stopped first
