import os
import plistlib
import sys
from unittest import mock

from sonari import cli


def test_launchagent_plist_embeds_resolved_python_and_pythonpath(tmp_path):
    log = "/home/u/.sonari/speechd.log"
    fake_python = "/usr/bin/python3"
    src = "/Users/u/.claude/plugins/sonari/src"
    xml = cli._launchagent_plist(python_executable=fake_python,
                                 src_path=src, log_path=log)
    assert isinstance(xml, str)
    assert xml.startswith("<?xml")
    data = plistlib.loads(xml.encode("utf-8"))
    assert data["Label"] == cli.LAUNCH_AGENT_LABEL
    assert data["ProgramArguments"] == [fake_python, "-m", "sonari.daemon"]
    assert data["EnvironmentVariables"]["PYTHONPATH"] == src
    assert data["RunAtLoad"] is True
    assert data["KeepAlive"] is True
    assert data["StandardErrorPath"] == log
    assert data["StandardOutPath"] == log
    # First arg must be an absolute interpreter path, never a bare name.
    interpreter = data["ProgramArguments"][0]
    assert os.path.isabs(interpreter)
    assert interpreter not in ("python3", "python")


def test_plist_xml_escapes_special_chars_in_paths():
    """A plugin path containing & / space / < must not corrupt the plist; the
    parsed PYTHONPATH must equal the original string intact."""
    log = "/home/u/.sonari/speechd.log"
    fake_python = "/usr/bin/python3"
    src = "/Users/u/My Plugins/A & B/<sonari>/src"
    xml = cli._launchagent_plist(python_executable=fake_python,
                                 src_path=src, log_path=log)
    # Raw XML must not contain a bare unescaped '&' or '<' inside the src value.
    assert "A & B" not in xml  # the bare ampersand was escaped
    assert "&amp;" in xml
    data = plistlib.loads(xml.encode("utf-8"))
    assert data["EnvironmentVariables"]["PYTHONPATH"] == src


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


def test_write_install_record_writes_expected_keys(tmp_path):
    rec = tmp_path / "install.json"
    with mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", rec):
        cli._write_install_record(
            python="/usr/bin/python3",
            python_version="3.9",
            plugin_root="/plug",
            src="/plug/src",
        )
    import json as _json
    data = _json.loads(rec.read_text())
    assert data["python"] == "/usr/bin/python3"
    assert data["python_version"] == "3.9"
    assert data["plugin_root"] == "/plug"
    assert data["src"] == "/plug/src"
    assert "installed_at" in data and isinstance(data["installed_at"], str)
