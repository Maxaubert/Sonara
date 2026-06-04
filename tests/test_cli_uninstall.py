import json
import os
from unittest import mock

from echo import cli


def test_uninstall_removes_launchagent_and_echo_dir(tmp_path):
    plist = tmp_path / "com.echo.speechd.plist"
    plist.write_text("<plist/>")
    echo_dir = tmp_path / ".echo"
    echo_dir.mkdir()
    (echo_dir / "config.json").write_text("{}")

    run = mock.Mock(return_value=0)
    with mock.patch.object(cli, "LAUNCH_AGENT_PATH", str(plist)), \
         mock.patch.object(cli, "_launchctl", run), \
         mock.patch.object(cli.paths, "ECHO_DIR", echo_dir), \
         mock.patch.object(cli, "_legacy_migrate", return_value=[]) as mig:
        rc = cli.uninstall()

    assert rc == 0
    assert not plist.exists()
    assert not echo_dir.exists()
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
    with mock.patch("echo.cli.uninstall", return_value=0) as un:
        rc = cli.main(["uninstall"])
    un.assert_called_once()
    assert rc == 0
