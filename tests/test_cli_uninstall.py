from unittest import mock

from sonari import cli
from tests._fakeplatform import fake_platform, FakeSupervisor, FakeHotkey


def test_uninstall_dispatches_and_cleans_shared_artifacts_preserving_user_files(
        tmp_path, monkeypatch):
    sonari_dir = tmp_path / ".sonari"
    sonari_dir.mkdir()
    # Runtime artifacts uninstall should remove (shared cleanup in cli).
    log = sonari_dir / "speechd.log"; log.write_text("log")
    resolved = sonari_dir / "hotkeyd.resolved.json"; resolved.write_text("[]")
    record = sonari_dir / "install.json"; record.write_text("{}")
    lock = sonari_dir / "daemon.lock"; lock.write_text("{}")
    hk_log = sonari_dir / "hotkeyd.log"; hk_log.write_text("x")
    app_dir = sonari_dir / "app"
    (app_dir / "sonari").mkdir(parents=True)
    (app_dir / "sonari" / "__init__.py").write_text("# pkg\n")
    # PRESERVED across uninstall: user keymap AND config.
    keymap = sonari_dir / "keymap.json"; keymap.write_text('{"custom": true}')
    config = sonari_dir / "config.json"; config.write_text('{"rate": 180}')

    sup = FakeSupervisor()
    hk = FakeHotkey()
    monkeypatch.setattr(cli, "_platform", lambda: fake_platform(supervisor=sup, hotkey=hk))
    with mock.patch.object(cli.paths, "SONARI_DIR", sonari_dir), \
         mock.patch.object(cli.paths, "CONFIG_PATH", config), \
         mock.patch.object(cli.paths, "LOG_PATH", log), \
         mock.patch.object(cli.paths, "LOCK_PATH", lock), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", keymap), \
         mock.patch.object(cli.paths, "INSTALL_RECORD_PATH", record), \
         mock.patch.object(cli.paths, "APP_DIR", app_dir):
        rc = cli.uninstall()

    assert rc == 0
    # Backend teardown was dispatched.
    assert ("uninstall",) in sup.calls
    assert ("uninstall",) in hk.calls
    # Shared artifacts gone.
    assert not log.exists() and not resolved.exists() and not record.exists()
    assert not lock.exists() and not hk_log.exists() and not app_dir.exists()
    # Preserved.
    assert keymap.exists() and keymap.read_text() == '{"custom": true}'
    assert config.exists() and config.read_text() == '{"rate": 180}'


def test_uninstall_subcommand_invokes_uninstall():
    with mock.patch("sonari.cli.uninstall", return_value=0) as un:
        rc = cli.main(["uninstall"])
    un.assert_called_once()
    assert rc == 0
