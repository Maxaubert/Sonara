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
