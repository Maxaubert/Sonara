"""Tests for legacy zshrc cleaner and settings.json cleaner in echo.cli."""
from pathlib import Path

from echo import cli


LEGACY_ZSHRC = """\
export EDITOR=vim
export PATH="$HOME/bin:$PATH"

# claude-tts
alias claude='claude-speak'

export PATH="$HOME/.local/bin:$PATH"  # claude-tts
alias gs='git status'
"""


def test_clean_zshrc_removes_legacy_lines(tmp_path):
    rc = tmp_path / ".zshrc"
    rc.write_text(LEGACY_ZSHRC)
    changed = cli._clean_zshrc(str(rc))
    assert changed is True
    text = rc.read_text()
    assert "claude-tts" not in text
    assert "claude-speak" not in text
    assert ".local/bin" not in text
    # Untouched lines survive.
    assert "export EDITOR=vim" in text
    assert 'export PATH="$HOME/bin:$PATH"' in text
    assert "alias gs='git status'" in text


def test_clean_zshrc_keeps_user_local_bin_without_marker(tmp_path):
    rc = tmp_path / ".zshrc"
    rc.write_text('export PATH="$HOME/.local/bin:$PATH"\nalias ll=\'ls -la\'\n')
    changed = cli._clean_zshrc(str(rc))
    assert changed is False
    assert ".local/bin" in rc.read_text()
    assert "alias ll='ls -la'" in rc.read_text()


def test_clean_zshrc_missing_file_is_noop(tmp_path):
    rc = tmp_path / "nope.zshrc"
    assert cli._clean_zshrc(str(rc)) is False
    assert not rc.exists()


def test_clean_zshrc_idempotent(tmp_path):
    rc = tmp_path / ".zshrc"
    rc.write_text(LEGACY_ZSHRC)
    assert cli._clean_zshrc(str(rc)) is True
    assert cli._clean_zshrc(str(rc)) is False


import json


def _legacy_settings():
    return {
        "model": "opus",
        "hooks": {
            "PreToolUse": [
                {"hooks": [{"type": "command",
                            "command": "/Users/x/.claude/hooks/claude-tts-pre-tool.sh"}]},
                {"hooks": [{"type": "command", "command": "/Users/x/keep-me.sh"}]},
            ],
            "Stop": [
                {"hooks": [{"type": "command",
                            "command": "/Users/x/.claude/hooks/claude-tts-stop.sh"}]},
            ],
            "PermissionRequest": [
                {"hooks": [{"type": "command",
                            "command": "/Users/x/.claude/hooks/claude-tts-permission.sh"}]},
            ],
        },
    }


def test_clean_settings_removes_legacy_hooks(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps(_legacy_settings()))
    changed = cli._clean_settings_json(str(sp))
    assert changed is True
    data = json.loads(sp.read_text())
    blob = json.dumps(data)
    assert "claude-tts" not in blob
    # Unrelated hook preserved; empty events dropped.
    assert data["hooks"]["PreToolUse"] == [
        {"hooks": [{"type": "command", "command": "/Users/x/keep-me.sh"}]}]
    assert "Stop" not in data["hooks"]
    assert "PermissionRequest" not in data["hooks"]
    assert data["model"] == "opus"


def test_clean_settings_missing_file_is_noop(tmp_path):
    sp = tmp_path / "settings.json"
    assert cli._clean_settings_json(str(sp)) is False


def test_clean_settings_corrupt_file_is_noop(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text("{not json")
    assert cli._clean_settings_json(str(sp)) is False
    # File left as-is when it cannot be parsed.
    assert sp.read_text() == "{not json"


def test_clean_settings_no_legacy_no_change(tmp_path):
    sp = tmp_path / "settings.json"
    sp.write_text(json.dumps({"hooks": {"Stop": [
        {"hooks": [{"type": "command", "command": "/Users/x/other.sh"}]}]}}))
    assert cli._clean_settings_json(str(sp)) is False
