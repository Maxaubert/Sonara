import os
import plistlib
import sys
from unittest import mock

from sonari import cli


def test_launchagent_plist_is_valid_and_complete(tmp_path):
    daemon = "/repo/bin/sonari-daemon"
    log = "/home/u/.sonari/speechd.log"
    fake_python = "/usr/local/venv/bin/python3"
    xml = cli._launchagent_plist(daemon, log, python_executable=fake_python)
    assert isinstance(xml, str)
    assert xml.startswith("<?xml")
    data = plistlib.loads(xml.encode("utf-8"))
    assert data["Label"] == cli.LAUNCH_AGENT_LABEL
    assert data["ProgramArguments"] == [fake_python, "-m", "sonari.daemon"]
    assert data["RunAtLoad"] is True
    assert data["KeepAlive"] is True
    assert data["StandardErrorPath"] == log
    assert data["StandardOutPath"] == log


def test_launchagent_plist_uses_absolute_python_not_bare_python3(tmp_path):
    """ProgramArguments must start with an absolute interpreter path.

    launchd runs agents with a minimal PATH that may not include the venv.
    Using bare 'python3' would silently invoke the wrong interpreter (or
    fail), so the first element of ProgramArguments must be an absolute path.
    """
    daemon = "/repo/bin/sonari-daemon"
    log = "/home/u/.sonari/speechd.log"

    # Default: no python_executable supplied — must fall back to sys.executable.
    xml = cli._launchagent_plist(daemon, log)
    data = plistlib.loads(xml.encode("utf-8"))
    prog_args = data["ProgramArguments"]
    interpreter = prog_args[0]

    # Must be absolute (starts with '/').
    assert os.path.isabs(interpreter), (
        f"ProgramArguments[0] is not an absolute path: {interpreter!r}"
    )
    # Must NOT be a bare name like 'python3'.
    assert interpreter not in ("python3", "python", "python3.x"), (
        f"ProgramArguments[0] must not be a bare interpreter name: {interpreter!r}"
    )
    # Must match the current sys.executable so the installed package is found.
    assert interpreter == sys.executable, (
        f"Expected sys.executable {sys.executable!r}, got {interpreter!r}"
    )
    # Module invocation must follow.
    assert prog_args[1:] == ["-m", "sonari.daemon"], (
        f"Expected ['-m', 'sonari.daemon'] after interpreter, got {prog_args[1:]!r}"
    )


def test_install_writes_plist_and_loads(tmp_path, capsys):
    la_dir = tmp_path / "LaunchAgents"
    plist = la_dir / (cli.LAUNCH_AGENT_LABEL + ".plist")
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(tmp_path / "com.sonari.hotkeyd.plist")), \
         mock.patch.object(cli, "_build_hotkeyd", return_value=(True, "built")), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir", lambda: tmp_path.mkdir(parents=True, exist_ok=True)), \
         mock.patch("sonari.paths.ensure_sonari_dir") as ensure:
        rc = cli.install()
    assert rc == 0
    ensure.assert_called_once()
    assert plist.exists()
    # launchctl unload (ignored) then load was attempted.
    assert any(c.args[0][0] == "load" for c in run.call_args_list)
    out = capsys.readouterr().out
    assert "/plugin" in out or "plugin" in out.lower()


def test_install_subcommand_invokes_install():
    with mock.patch("sonari.cli.install", return_value=0) as inst:
        rc = cli.main(["install"])
    inst.assert_called_once()
    assert rc == 0
