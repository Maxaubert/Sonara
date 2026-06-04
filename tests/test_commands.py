import os

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CMD = os.path.join(REPO, "commands")


def _read(name):
    with open(os.path.join(CMD, name), encoding="utf-8") as f:
        return f.read()


def test_all_command_files_exist():
    for name in ("echo:status.md", "echo:verbosity.md", "echo:stop.md",
                 "echo:repeat.md", "echo:doctor.md"):
        assert os.path.exists(os.path.join(CMD, name)), name


def test_status_runs_status_and_shows_output():
    txt = _read("echo:status.md")
    assert "echo status" in txt
    assert "Bash" in txt
    # status surfaces output to the user.
    assert "print" in txt.lower()


def test_verbosity_passes_argument_and_is_silent():
    txt = _read("echo:verbosity.md")
    assert "echo verbosity" in txt
    assert "$ARGUMENTS" in txt or "ARGUMENTS" in txt
    assert "nothing" in txt.lower()


def test_stop_is_silent():
    txt = _read("echo:stop.md")
    assert "echo stop" in txt
    assert "nothing" in txt.lower()


def test_repeat_is_silent():
    txt = _read("echo:repeat.md")
    assert "echo repeat" in txt
    assert "nothing" in txt.lower()


def test_doctor_shows_output():
    txt = _read("echo:doctor.md")
    assert "echo doctor" in txt
    assert "Bash" in txt
    assert "print" in txt.lower()
