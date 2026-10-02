"""The plugin's slash commands (commands/*.md) run bin/sonara, the bash
wrapper around the runtime's sonara.exe (#202): no Python anywhere."""
import os
import re

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CMD = os.path.join(REPO, "commands")

# Only the lifecycle ESSENTIALS ship as slash commands (user decision,
# 2026-07-24): day-to-day tuning lives in the settings page. Files use
# NTFS-safe names (no colon). `install` is gone with #202: the first hook
# (or any command) installs the runtime.
COMMANDS = ("doctor", "settings", "start", "uninstall")
# Everything that must NOT ship a command file: hotkey mirrors, CLI-only
# verbs (`sonara stop`), the settings-page-superseded tuning commands
# removed 2026-07-24, and the Python install step.
DROPPED = ("install", "stop", "skip", "repeat", "status", "verbosity", "voice", "voices",
           "rate", "minqueue", "summary", "audio-control", "audio-mode",
           "duck-level", "keymap", "volume")


def _read(name):
    with open(os.path.join(CMD, name), encoding="utf-8") as f:
        return f.read()


def test_only_the_essential_command_files_exist():
    shipped = sorted(f[:-3] for f in os.listdir(CMD) if f.endswith(".md"))
    assert shipped == sorted(COMMANDS)


def test_no_colon_named_or_dropped_command_files():
    for verb in COMMANDS + DROPPED:
        assert not os.path.exists(os.path.join(CMD, "sonara:" + verb + ".md")), verb
    for verb in DROPPED:
        assert not os.path.exists(os.path.join(CMD, verb + ".md")), verb


def test_every_command_invokes_its_verb_through_the_wrapper():
    for verb in COMMANDS:
        txt = _read(verb + ".md")
        assert 'bash "${CLAUDE_PLUGIN_ROOT}/bin/sonara" ' + verb in txt, verb
        assert "Bash tool" in txt, verb
        assert txt.lstrip().startswith("---"), verb   # YAML front-matter
        assert "description:" in txt, verb
        assert "verbatim" in txt.lower() or "tell the user" in txt.lower(), verb


def test_no_command_runs_python_or_powershell_setup():
    for name in os.listdir(CMD):
        txt = _read(name).lower()
        assert not re.search(r"\bpython[w3]?\b|\bpy -3\b|\bpip\b", txt), name
        assert "sonara-bootstrap.ps1" not in txt, name


def test_uninstall_asks_what_to_keep():
    txt = _read("uninstall.md")
    assert "AskUserQuestion" in txt
    assert "--keep" in txt
    for item in ("settings", "models", "logs", "none"):
        assert item in txt, item
    assert "/plugin uninstall sonara@sonara" in txt


def test_settings_does_not_repeat_the_token_url():
    assert "token" in _read("settings.md")


def test_commands_have_no_em_dash():
    for name in os.listdir(CMD):
        assert "—" not in _read(name), name
