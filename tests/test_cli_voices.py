import os

import pytest
from sonara import cli, paths
from sonara import kokoro_provision as kp


def test_voices_install_provisions_then_rewires_daemon(monkeypatch, tmp_path):
    order = []
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "package_root", lambda: os.path.join(str(tmp_path), "src"))
    monkeypatch.setattr(kp, "install_kokoro", lambda pythonpath: order.append(("provision", pythonpath)))
    monkeypatch.setattr(kp, "neural_healthy", lambda app_dir: True)
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    rc = cli._cmd_voices_install(object())
    assert rc == 0
    assert order == [("provision", str(tmp_path / "src")), "install"]


def test_voices_install_passes_package_root_not_app_dir_to_install_kokoro(monkeypatch, tmp_path):
    """install_kokoro must receive the running package root (repo src/ in a
    checkout) so predownload can import sonara even before install()
    populates APP_DIR."""
    received = []
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "package_root", lambda: os.path.join(str(tmp_path), "src"))
    monkeypatch.setattr(kp, "install_kokoro", lambda pythonpath: received.append(pythonpath))
    monkeypatch.setattr(kp, "neural_healthy", lambda app_dir: True)
    monkeypatch.setattr(cli, "install", lambda: 0)
    cli._cmd_voices_install(object())
    assert received == [os.path.join(str(tmp_path), "src")]


def test_voices_install_reports_failure_without_rewiring(monkeypatch, tmp_path):
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "package_root", lambda: os.path.join(str(tmp_path), "src"))
    uninstall_called = []
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: uninstall_called.append(True))
    def boom(pythonpath): raise RuntimeError("uv missing")
    monkeypatch.setattr(kp, "install_kokoro", boom)
    monkeypatch.setattr(cli, "install", lambda: pytest.fail("must not rewire on failure"))
    rc = cli._cmd_voices_install(object())
    assert rc == 1
    assert uninstall_called, "uninstall_kokoro must be called on failure to revert half-built state"


def test_voices_install_reverts_on_keyboard_interrupt(monkeypatch, tmp_path):
    # Ctrl+C during the download must still revert the half-built venv, which
    # would otherwise read as fully provisioned forever (venv python exists after
    # step 1 of a multi-GB install) -- `except Exception` missed it (audit #21).
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "package_root", lambda: os.path.join(str(tmp_path), "src"))
    uninstalled = []
    def boom(pythonpath):
        raise KeyboardInterrupt()
    monkeypatch.setattr(kp, "install_kokoro", boom)
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: uninstalled.append(True))
    monkeypatch.setattr(cli, "install", lambda: pytest.fail("must not rewire"))
    with pytest.raises(KeyboardInterrupt):               # interrupt still propagates
        cli._cmd_voices_install(object())
    assert uninstalled


def test_voices_uninstall_stops_daemon_first(monkeypatch):
    # The venv being removed is the interpreter the daemon runs on; deleting
    # it live raised a raw PermissionError and left a half-deleted venv (audit #23). Stop first.
    order = []
    monkeypatch.setattr(cli, "stop_sonara",
                        lambda sup=None: order.append("stop") or True)
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: order.append("rm"))
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    cli._cmd_voices_uninstall(object())
    assert order.index("stop") < order.index("rm")


def test_voices_uninstall_removes_and_reverts(monkeypatch):
    order = []
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: order.append("rm"))
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    rc = cli._cmd_voices_uninstall(object())
    assert rc == 0
    assert order == ["rm", "install"]   # remove venv, then re-wire to system 3.9


def test_voices_subcommand_registered():
    parser = cli._build_parser()
    args = parser.parse_args(["voices", "install"])
    assert args.func is cli._cmd_voices_install
    assert args.engine == "kokoro"


def test_voices_subcommand_rejects_the_removed_chatterbox_engine():
    parser = cli._build_parser()
    with pytest.raises(SystemExit):
        parser.parse_args(["voices", "install", "chatterbox"])


# ---------------------------------------------------------------------------
# Task 5: engine dispatch (kokoro default)
# ---------------------------------------------------------------------------

def test_voices_install_default_still_kokoro(monkeypatch, tmp_path):
    """cli.main(["voices", "install"]) with no engine arg keeps calling
    kokoro_provision.install_kokoro (backward compatible)."""
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "package_root", lambda: os.path.join(str(tmp_path), "src"))
    called = []
    monkeypatch.setattr(kp, "install_kokoro", lambda pythonpath: called.append(pythonpath))
    monkeypatch.setattr(kp, "neural_healthy", lambda app_dir: True)
    monkeypatch.setattr(cli, "install", lambda: 0)
    rc = cli.main(["voices", "install"])
    assert rc == 0
    assert called == [os.path.join(str(tmp_path), "src")]


def test_voices_uninstall_default_still_kokoro(monkeypatch):
    order = []
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: order.append("rm"))
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    rc = cli.main(["voices", "uninstall"])
    assert rc == 0
    assert order == ["rm", "install"]


# ---------------------------------------------------------------------------
# E10: install against a stopped daemon, never delete a working venv
# ---------------------------------------------------------------------------

def _voices_env(monkeypatch, tmp_path, order, *, existed, running=False):
    monkeypatch.setattr(paths, "APP_DIR", tmp_path / "app")
    monkeypatch.setattr(paths, "package_root", lambda: os.path.join(str(tmp_path), "src"))
    monkeypatch.setattr(kp, "neural_enabled", lambda: existed)
    monkeypatch.setattr(kp, "neural_healthy", lambda app_dir: True)
    monkeypatch.setattr(paths, "socket_connectable", lambda: running)
    monkeypatch.setattr(cli, "stop_sonara", lambda sup=None: order.append("stop") or True)
    monkeypatch.setattr(cli, "start_sonara", lambda: order.append("start") or 0)
    monkeypatch.setattr(kp, "uninstall_kokoro", lambda: order.append("rm"))


def test_voices_install_stops_the_daemon_before_provisioning(monkeypatch, tmp_path):
    """The daemon may run on the venv's pythonw: provisioning under it hit
    locked files (E10)."""
    order = []
    _voices_env(monkeypatch, tmp_path, order, existed=True, running=True)
    monkeypatch.setattr(kp, "install_kokoro", lambda pythonpath: order.append("provision"))
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 0)
    assert cli._cmd_voices_install(object()) == 0
    assert order.index("stop") < order.index("provision") < order.index("install")


def test_voices_install_keeps_a_working_venv_when_it_fails(monkeypatch, tmp_path, capsys):
    """A re-run or upgrade that fails (say, a network error in the predownload)
    must leave the previously healthy venv in place (E10)."""
    order = []
    _voices_env(monkeypatch, tmp_path, order, existed=True)

    def boom(pythonpath):
        raise RuntimeError("network down")
    monkeypatch.setattr(kp, "install_kokoro", boom)
    monkeypatch.setattr(cli, "install", lambda: pytest.fail("must not rewire"))
    assert cli._cmd_voices_install(object()) == 1
    assert "rm" not in order
    err = capsys.readouterr().err
    assert "network down" in err and "kept" in err.lower()


def test_voices_install_failure_restarts_a_daemon_that_was_running(monkeypatch, tmp_path):
    order = []
    _voices_env(monkeypatch, tmp_path, order, existed=True, running=True)

    def boom(pythonpath):
        raise RuntimeError("network down")
    monkeypatch.setattr(kp, "install_kokoro", boom)
    assert cli._cmd_voices_install(object()) == 1
    assert order[-1] == "start"


def test_voices_install_failure_does_not_leave_sonara_shut_down(monkeypatch, tmp_path):
    """Not running before (lazy start by hooks): the stop sentinel this
    command wrote is removed again, so hooks can start it."""
    order = []
    _voices_env(monkeypatch, tmp_path, order, existed=False)
    paths.ensure_sonara_dir()

    def boom(pythonpath):
        with open(str(paths.STOPPED_SENTINEL_PATH), "w") as fh:
            fh.write("x")                 # what the real stop_sonara leaves
        raise RuntimeError("uv missing")
    monkeypatch.setattr(kp, "install_kokoro", boom)
    assert cli._cmd_voices_install(object()) == 1
    assert not os.path.exists(str(paths.STOPPED_SENTINEL_PATH))


def test_voices_install_revert_error_does_not_hide_the_cause(monkeypatch, tmp_path, capsys):
    order = []
    _voices_env(monkeypatch, tmp_path, order, existed=False)

    def boom(pythonpath):
        raise RuntimeError("uv venv failed")

    def locked():
        raise PermissionError("python.exe is in use")
    monkeypatch.setattr(kp, "install_kokoro", boom)
    monkeypatch.setattr(kp, "uninstall_kokoro", locked)
    assert cli._cmd_voices_install(object()) == 1
    assert "uv venv failed" in capsys.readouterr().err


def test_voices_install_restarts_a_running_daemon_when_install_fails_early(monkeypatch, tmp_path):
    """install() can return 1 before its own stop and sentinel-clearing
    finally (no Python, no plugin tree): the stop this command did must be
    undone, or Sonara stays off."""
    order = []
    _voices_env(monkeypatch, tmp_path, order, existed=True, running=True)
    monkeypatch.setattr(kp, "install_kokoro", lambda pythonpath: order.append("provision"))
    monkeypatch.setattr(cli, "install", lambda: order.append("install") or 1)
    assert cli._cmd_voices_install(object()) == 1
    assert order[-1] == "start"


def test_voices_install_early_install_failure_clears_the_stop_sentinel(monkeypatch, tmp_path):
    order = []
    _voices_env(monkeypatch, tmp_path, order, existed=False)
    paths.ensure_sonara_dir()

    def provision(pythonpath):
        with open(str(paths.STOPPED_SENTINEL_PATH), "w") as fh:
            fh.write("x")                 # what the real stop_sonara leaves
    monkeypatch.setattr(kp, "install_kokoro", provision)
    monkeypatch.setattr(cli, "install", lambda: 1)
    assert cli._cmd_voices_install(object()) == 1
    assert not os.path.exists(str(paths.STOPPED_SENTINEL_PATH))
