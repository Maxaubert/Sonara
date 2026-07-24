import os

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CMD = os.path.join(REPO, "commands")

# Only the lifecycle ESSENTIALS ship as slash commands (user decision,
# 2026-07-24): day-to-day tuning lives in the settings page, and the removed
# verbs remain available as CLI subcommands. Files use NTFS-safe names (no
# colon). `install` routes through the PowerShell bootstrap (does NOT match
# the launcher pattern) and is asserted in test_bin_shims.py; `settings` and
# `start` carry extra guidance text, asserted below.
COMMANDS = ("doctor", "settings", "start", "uninstall")
# Everything that must NOT ship a command file: hotkey mirrors, CLI-only
# verbs, and the settings-page-superseded tuning commands removed 2026-07-24.
DROPPED = ("stop", "skip", "repeat", "status", "verbosity", "voice", "voices",
           "rate", "minqueue", "summary", "audio-control", "audio-mode",
           "duck-level", "keymap", "volume")


def _read(name):
    with open(os.path.join(CMD, name), encoding="utf-8") as f:
        return f.read()


def test_only_the_essential_command_files_exist():
    for verb in COMMANDS + ("install",):
        assert os.path.exists(os.path.join(CMD, verb + ".md")), verb
    shipped = sorted(f[:-3] for f in os.listdir(CMD) if f.endswith(".md"))
    assert shipped == sorted(COMMANDS + ("install",))


def test_no_colon_named_or_dropped_command_files():
    # Colons are illegal on NTFS: every command file was renamed off the colon
    # form, and the hotkey-mirror / CLI-only / settings-page verbs ship no
    # file at all. This locks in both rules so a regression can't reintroduce
    # colon names or a removed command.
    for verb in COMMANDS + DROPPED:
        assert not os.path.exists(os.path.join(CMD, "sonara:" + verb + ".md")), verb
    for verb in DROPPED:
        assert not os.path.exists(os.path.join(CMD, verb + ".md")), verb


def test_every_command_invokes_its_verb_through_the_launcher():
    for verb in COMMANDS:
        txt = _read(verb + ".md")
        assert 'bin/sonara" {0}'.format(verb) in txt, verb
        assert "Bash tool" in txt, verb
        assert txt.lstrip().startswith("---"), verb   # YAML front-matter
        assert "description:" in txt, verb
        assert "print" in txt.lower(), verb           # surfaces output to the user


def test_doctor_surfaces_output_verbatim():
    assert "verbatim" in _read("doctor.md").lower()
