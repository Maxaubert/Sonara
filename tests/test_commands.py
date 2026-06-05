import os

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CMD = os.path.join(REPO, "commands")


def _read(name):
    with open(os.path.join(CMD, name), encoding="utf-8") as f:
        return f.read()


def test_all_command_files_exist():
    for name in ("sonari:status.md", "sonari:verbosity.md", "sonari:stop.md",
                 "sonari:repeat.md", "sonari:doctor.md", "sonari:keymap.md"):
        assert os.path.exists(os.path.join(CMD, name)), name


def test_status_runs_status_and_shows_output():
    txt = _read("sonari:status.md")
    assert "sonari status" in txt
    assert "Bash" in txt
    # status surfaces output to the user.
    assert "print" in txt.lower()


def test_verbosity_passes_argument_and_is_silent():
    txt = _read("sonari:verbosity.md")
    assert "sonari verbosity" in txt
    assert "$ARGUMENTS" in txt or "ARGUMENTS" in txt
    assert "nothing" in txt.lower()


def test_stop_is_silent():
    txt = _read("sonari:stop.md")
    assert "sonari stop" in txt
    assert "nothing" in txt.lower()


def test_repeat_is_silent():
    txt = _read("sonari:repeat.md")
    assert "sonari repeat" in txt
    assert "nothing" in txt.lower()


def test_doctor_shows_output():
    txt = _read("sonari:doctor.md")
    assert "sonari doctor" in txt
    assert "Bash" in txt
    assert "print" in txt.lower()


def test_keymap_command_file_exists_and_runs_sonari_keymap():
    assert os.path.exists(os.path.join(CMD, "sonari:keymap.md"))
    txt = _read("sonari:keymap.md")
    assert "sonari keymap" in txt
    assert "Bash" in txt
    assert "print" in txt.lower()
