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
    record = tmp_path / "install.json"
    app_dir = tmp_path / ".sonari" / "app"
    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_probe_python_version", return_value=(3, 9)), \
         mock.patch.object(cli, "_build_hotkeyd", return_value=(True, "built")), \
         mock.patch.object(cli, "_copy_app", return_value=str(app_dir)) as copy_app, \
         mock.patch.object(cli, "_read_plugin_version", return_value="0.4.0"), \
         mock.patch.object(cli, "_place_launcher", return_value=str(tmp_path / "launcher")) as place_launcher, \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(tmp_path / "com.sonari.hotkeyd.plist")), \
         mock.patch.object(cli.paths, "APP_DIR", app_dir), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
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
    copy_app.assert_called_once()
    assert plist.exists()
    # The speechd plist embeds the resolved interpreter + PYTHONPATH=<APP_DIR>.
    data = plistlib.loads(plist.read_text().encode("utf-8"))
    assert data["ProgramArguments"][0] == "/usr/bin/python3"
    assert data["ProgramArguments"][1:] == ["-m", "sonari.daemon"]
    assert data["EnvironmentVariables"]["PYTHONPATH"] == str(app_dir)
    # install.json was written with the resolved interpreter + new fields.
    import json as _json
    rec = _json.loads(record.read_text())
    assert rec["python"] == "/usr/bin/python3"
    assert rec["app_path"] == str(app_dir)
    assert rec["plugin_version"] == "0.4.0"
    assert "src" not in rec
    place_launcher.assert_called_once()
    assert any(c.args[0][0] == "load" for c in run.call_args_list)
    out = capsys.readouterr().out
    assert "doctor" in out.lower()


def test_install_fatal_when_no_python_found(capsys):
    with mock.patch.object(cli, "_resolve_python", return_value=None), \
         mock.patch("sonari.paths.ensure_sonari_dir"):
        rc = cli.install()
    assert rc != 0
    out = capsys.readouterr().out
    assert "python3" in out.lower()
    assert "xcode-select --install" in out.lower()


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
            app_path="/home/u/.sonari/app",
            plugin_version="0.4.0",
        )
    import json as _json
    data = _json.loads(rec.read_text())
    assert data["python"] == "/usr/bin/python3"
    assert data["python_version"] == "3.9"
    assert data["plugin_root"] == "/plug"
    assert data["app_path"] == "/home/u/.sonari/app"
    assert data["plugin_version"] == "0.4.0"
    assert "src" not in data  # src key was replaced by app_path
    assert "installed_at" in data and isinstance(data["installed_at"], str)


def test_read_plugin_version_reads_version_from_plugin_json(tmp_path, monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    pdir = tmp_path / ".claude-plugin"
    pdir.mkdir()
    (pdir / "plugin.json").write_text('{"name": "sonari", "version": "0.4.0"}')
    assert cli._read_plugin_version(str(tmp_path)) == "0.4.0"


def test_read_plugin_version_missing_file_returns_empty(tmp_path, monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    assert cli._read_plugin_version(str(tmp_path)) == ""


def test_read_plugin_version_corrupt_file_returns_empty(tmp_path, monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    pdir = tmp_path / ".claude-plugin"
    pdir.mkdir()
    (pdir / "plugin.json").write_text("{ not json")
    assert cli._read_plugin_version(str(tmp_path)) == ""


def test_read_plugin_version_falls_back_to_env(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_PLUGIN_VERSION", "9.9.9")
    # No plugin.json on disk -> env fallback wins.
    assert cli._read_plugin_version(str(tmp_path)) == "9.9.9"


def test_copy_app_copies_package_into_app_dir(tmp_path):
    plugin_root = tmp_path / "plugin"
    src_pkg = plugin_root / "src" / "sonari"
    src_pkg.mkdir(parents=True)
    (src_pkg / "__init__.py").write_text("# sonari\n")
    (src_pkg / "daemon.py").write_text("# daemon\n")
    app_dir = tmp_path / "home" / ".sonari" / "app"
    with mock.patch.object(cli.paths, "APP_DIR", app_dir):
        returned = cli._copy_app(str(plugin_root))
    assert returned == str(app_dir)
    assert (app_dir / "sonari" / "__init__.py").exists()
    assert (app_dir / "sonari" / "daemon.py").exists()


def test_copy_app_is_remove_then_copy_so_stale_modules_vanish(tmp_path):
    app_dir = tmp_path / "home" / ".sonari" / "app"

    def _root_with(modules):
        root = tmp_path / ("plug-" + "-".join(modules))
        pkg = root / "src" / "sonari"
        pkg.mkdir(parents=True)
        (pkg / "__init__.py").write_text("# pkg\n")
        for m in modules:
            (pkg / m).write_text("# " + m + "\n")
        return root

    first = _root_with(["old_only.py", "daemon.py"])
    second = _root_with(["daemon.py"])
    with mock.patch.object(cli.paths, "APP_DIR", app_dir):
        cli._copy_app(str(first))
        assert (app_dir / "sonari" / "old_only.py").exists()
        cli._copy_app(str(second))
    # The module present only in the FIRST root must be gone after re-copy.
    assert not (app_dir / "sonari" / "old_only.py").exists()
    assert (app_dir / "sonari" / "daemon.py").exists()


def test_copy_app_raises_oserror_when_source_missing(tmp_path):
    plugin_root = tmp_path / "plugin"  # no src/sonari beneath it
    app_dir = tmp_path / "home" / ".sonari" / "app"
    with mock.patch.object(cli.paths, "APP_DIR", app_dir):
        try:
            cli._copy_app(str(plugin_root))
            raised = False
        except OSError:
            raised = True
    assert raised is True


def test_install_copy_failure_is_fatal_and_writes_no_plist(tmp_path, capsys):
    plist = tmp_path / "com.sonari.speechd.plist"
    record = tmp_path / "install.json"
    app_dir = tmp_path / ".sonari" / "app"
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", mock.Mock(return_value=0)), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_probe_python_version", return_value=(3, 9)), \
         mock.patch.object(cli, "_build_hotkeyd", return_value=(True, "built")), \
         mock.patch.object(cli, "_copy_app", side_effect=OSError("read-only")), \
         mock.patch.object(cli.paths, "APP_DIR", app_dir), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir", lambda: None), \
         mock.patch("sonari.paths.ensure_sonari_dir"):
        rc = cli.install()
    assert rc == 1
    # No speechd plist was written when the copy failed.
    assert not plist.exists()
    out = capsys.readouterr().out.lower()
    assert "~/.sonari" in out or ".sonari is writable" in out


def test_install_plist_pythonpath_handles_spaces_in_app_dir(tmp_path, capsys):
    plist = tmp_path / "com.sonari.speechd.plist"
    record = tmp_path / "install.json"
    # APP_DIR with a space in the path (e.g. a username with a space).
    app_dir = tmp_path / "Spaced Home" / ".sonari" / "app"
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", mock.Mock(return_value=0)), \
         mock.patch.object(cli, "_resolve_python", return_value="/usr/bin/python3"), \
         mock.patch.object(cli, "_probe_python_version", return_value=(3, 9)), \
         mock.patch.object(cli, "_build_hotkeyd", return_value=(False, "swiftc not found")), \
         mock.patch.object(cli, "_copy_app", return_value=str(app_dir)), \
         mock.patch.object(cli, "_read_plugin_version", return_value="0.4.0"), \
         mock.patch.object(cli, "_place_launcher", return_value=str(tmp_path / "launcher")), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(tmp_path / "hk.plist")), \
         mock.patch.object(cli.paths, "APP_DIR", app_dir), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", tmp_path / "sonari-hotkeyd"), \
         mock.patch.object(cli.keymap, "KEYMAP_PATH", tmp_path / "keymap.json"), \
         mock.patch.object(cli.keymap, "HOTKEYD_RESOLVED_PATH", tmp_path / "hotkeyd.resolved.json"), \
         mock.patch.object(cli.keymap, "SONARI_DIR", tmp_path), \
         mock.patch.object(cli.keymap, "ensure_sonari_dir", lambda: None), \
         mock.patch("sonari.paths.ensure_sonari_dir"):
        rc = cli.install()
    assert rc == 0
    data = plistlib.loads(plist.read_text().encode("utf-8"))
    # The spaced path round-trips through the plist XML intact.
    assert data["EnvironmentVariables"]["PYTHONPATH"] == str(app_dir)
    assert " " in data["EnvironmentVariables"]["PYTHONPATH"]
