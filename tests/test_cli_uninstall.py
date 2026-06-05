import json
import os
from unittest import mock

from sonari import cli


def test_uninstall_removes_launchagent_but_preserves_keymap(tmp_path):
    plist = tmp_path / "com.sonari.speechd.plist"
    plist.write_text("<plist/>")
    hotkeyd_plist = tmp_path / "com.sonari.hotkeyd.plist"
    hotkeyd_plist.write_text("<plist/>")
    sonari_dir = tmp_path / ".sonari"
    sonari_dir.mkdir()
    # Sonari-owned runtime artifacts that uninstall should remove.
    config = sonari_dir / "config.json"
    config.write_text("{}")
    log = sonari_dir / "speechd.log"
    log.write_text("log")
    resolved = sonari_dir / "hotkeyd.resolved.json"
    resolved.write_text("[]")
    binp = sonari_dir / "sonari-hotkeyd"
    binp.write_text("binary")
    # The user's customized hotkey rebindings: must survive uninstall (spec §5).
    keymap = sonari_dir / "keymap.json"
    keymap.write_text('{"custom": true}')

    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli.paths, "SONARI_DIR", sonari_dir), \
         mock.patch.object(cli.paths, "CONFIG_PATH", config), \
         mock.patch.object(cli.paths, "LOG_PATH", log), \
         mock.patch.object(cli.paths, "SOCKET_PATH", sonari_dir / "speechd.sock"), \
         mock.patch.object(cli.paths, "HOTKEYD_RESOLVED_PATH", resolved), \
         mock.patch.object(cli.paths, "KEYMAP_PATH", keymap), \
         mock.patch.object(cli, "HOTKEYD_LAUNCH_AGENT_PATH", str(hotkeyd_plist)), \
         mock.patch.object(cli.paths, "HOTKEYD_BIN_PATH", binp), \
         mock.patch.object(cli, "_legacy_migrate", return_value=[]) as mig:
        rc = cli.uninstall()

    assert rc == 0
    # LaunchAgents and binary are gone.
    assert not plist.exists()
    assert not hotkeyd_plist.exists()
    assert not binp.exists()
    # Runtime artifacts are gone.
    assert not config.exists()
    assert not log.exists()
    assert not resolved.exists()
    # keymap.json is preserved across uninstall.
    assert keymap.exists()
    assert keymap.read_text() == '{"custom": true}'
    assert any(c.args[0][0] == "unload" for c in run.call_args_list)
    mig.assert_called_once()


def test_legacy_migrate_cleans_everything(tmp_path):
    home = tmp_path
    zshrc = home / ".zshrc"
    zshrc.write_text("# claude-tts\nalias claude='claude-speak'\n"
                     'export PATH="$HOME/.local/bin:$PATH"  # claude-tts\n'
                     "export EDITOR=vim\n")
    claude = home / ".claude"
    claude.mkdir()
    settings = claude / "settings.json"
    settings.write_text(json.dumps({"hooks": {"Stop": [
        {"hooks": [{"type": "command",
                    "command": str(claude / "hooks/claude-tts-stop.sh")}]}]}}))
    local_bin = home / ".local" / "bin"
    local_bin.mkdir(parents=True)
    (local_bin / "claude-speak").write_text("x")
    (local_bin / "claude-tts").write_text("x")
    (home / ".claude-tts-enabled").write_text("1")
    (home / ".claude-tts-pos").write_text("0")

    removed = cli._legacy_migrate(home=str(home))

    assert "claude-tts" not in zshrc.read_text()
    assert "claude-tts" not in settings.read_text()
    assert not (local_bin / "claude-speak").exists()
    assert not (local_bin / "claude-tts").exists()
    assert not (home / ".claude-tts-enabled").exists()
    assert not (home / ".claude-tts-pos").exists()
    # removed is a human-readable list of what was cleaned.
    assert any("claude-speak" in r for r in removed)
    assert "export EDITOR=vim" in zshrc.read_text()


def test_legacy_migrate_on_clean_machine_is_safe(tmp_path):
    removed = cli._legacy_migrate(home=str(tmp_path))
    assert removed == [] or all(isinstance(r, str) for r in removed)


def test_uninstall_subcommand_invokes_uninstall():
    with mock.patch("sonari.cli.uninstall", return_value=0) as un:
        rc = cli.main(["uninstall"])
    un.assert_called_once()
    assert rc == 0
