from unittest import mock

from sonari import cli


def test_uninstall_removes_launchagent_but_preserves_keymap_and_config(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    plist = tmp_path / "com.sonari.speechd.plist"
    plist.write_text("<plist/>")
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    hotkeyd_plist.write_text("<plist/>")
    sonari_dir = tmp_path / ".sonari"
    sonari_dir.mkdir()
    # Runtime artifacts uninstall should remove.
    log = sonari_dir / "speechd.log"
    log.write_text("log")
    resolved = sonari_dir / "hotkeyd.resolved.json"
    resolved.write_text("[]")
    binp = sonari_dir / "sonari-hotkeyd"
    binp.write_text("binary")
    record = sonari_dir / "install.json"
    record.write_text("{}")
    lock = sonari_dir / "daemon.lock"
    lock.write_text("{}")
    # The stable app copy uninstall should remove.
    app_dir = sonari_dir / "app"
    (app_dir / "sonari").mkdir(parents=True)
    (app_dir / "sonari" / "__init__.py").write_text("# pkg\n")
    # PRESERVED across uninstall: user keymap AND config.
    keymap = sonari_dir / "keymap.json"
    keymap.write_text('{"custom": true}')
    config = sonari_dir / "config.json"
    config.write_text('{"rate": 180}')
    # The launcher uninstall should remove.
    local_bin = tmp_path / ".local" / "bin"
    local_bin.mkdir(parents=True)
    launcher = local_bin / "sonari"
    launcher.write_text("#!/bin/sh\n")

    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli.paths, "SONARI_DIR", sonari_dir), \
         mock.patch.object(cli.paths, "CONFIG_PATH", config), \
         mock.patch.object(cli.paths, "LOG_PATH", log), \
         mock.patch.object(cli.paths, "LOCK_PATH", lock), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", keymap), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "APP_DIR", app_dir), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp):
        rc = cli.uninstall()

    assert rc == 0
    assert not plist.exists()
    assert not hotkeyd_plist.exists()
    assert not binp.exists()
    assert not log.exists()
    assert not resolved.exists()
    assert not record.exists()
    assert not lock.exists()
    assert not app_dir.exists()
    assert not launcher.exists()
    # Preserved.
    assert keymap.exists()
    assert keymap.read_text() == '{"custom": true}'
    assert config.exists()
    assert config.read_text() == '{"rate": 180}'
    assert any(c.args[0][0] == "unload" for c in run.call_args_list)


def test_uninstall_subcommand_invokes_uninstall():
    with mock.patch("sonari.cli.uninstall", return_value=0) as un:
        rc = cli.main(["uninstall"])
    un.assert_called_once()
    assert rc == 0
