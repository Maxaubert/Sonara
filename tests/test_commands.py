import os

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CMD = os.path.join(REPO, "commands")

# The shipped slash-commands. Files use NTFS-safe names (no colon) so they check
# out and work on Windows too. stop/skip/repeat are pure hotkey mirrors, so they
# ship no command file. `uninstall` invokes the launcher like the rest; `install`
# is a real command too but routes through the PowerShell bootstrap (so it does
# NOT match the launcher pattern) and is asserted in test_bin_shims.py instead.
COMMANDS = ("status", "verbosity", "doctor", "keymap", "voice", "rate", "uninstall")
ARG_COMMANDS = ("verbosity", "voice", "rate", "keymap")  # forward $ARGUMENTS
DROPPED = ("stop", "skip", "repeat")


def _read(name):
    with open(os.path.join(CMD, name), encoding="utf-8") as f:
        return f.read()


def test_all_command_files_exist():
    for verb in COMMANDS:
        assert os.path.exists(os.path.join(CMD, verb + ".md")), verb


def test_no_colon_named_or_dropped_command_files():
    # Colons are illegal on NTFS: every command file was renamed off the colon
    # form, and the hotkey-mirror / CLI-only verbs ship no file at all. This locks
    # in the Windows-safe rename so a regression can't reintroduce colon names.
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


def test_argument_commands_forward_arguments():
    for verb in ARG_COMMANDS:
        txt = _read(verb + ".md")
        assert "$ARGUMENTS" in txt, verb


def test_status_and_doctor_surface_output_verbatim():
    for verb in ("status", "doctor"):
        txt = _read(verb + ".md")
        assert "verbatim" in txt.lower(), verb
