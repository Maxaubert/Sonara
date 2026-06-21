import os

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(REPO, "bin")


def _read(name):
    with open(os.path.join(BIN, name), encoding="utf-8") as f:
        return f.read()


def test_bootstrap_ps1_provisions_via_uv_and_hands_off():
    txt = _read("sonara-bootstrap.ps1")
    # downloads the uv BINARY from GitHub releases (no remote-script execution)
    assert "astral-sh/uv/releases/download" in txt
    assert "uv python install 3.12" in txt
    # rejects the Microsoft Store stub when probing system Python
    assert "WindowsApps" in txt
    # writes the interpreter record both consumers read
    assert "python.path" in txt and "pythonw.path" in txt
    # hands off to the real installer
    assert "sonara.cli install" in txt


def test_sonara_cmd_falls_back_to_recorded_python():
    assert "python.path" in _read("sonara.cmd")

def test_sonara_hook_cmd_falls_back_to_recorded_pythonw():
    assert "pythonw.path" in _read("sonara-hook.cmd")

def test_bin_sonara_bash_falls_back_to_recorded_python():
    assert "python.path" in _read("sonara")


def test_sonara_hook_cmd_resolves_interpreter_and_logs_stderr():
    """M10: the Windows hook launcher must not silently mute. It resolves a
    windowless interpreter (pythonw, with a `pyw -3` fallback) and appends stderr
    to ~/.sonara/hook.log instead of discarding it — while still exiting 0 so a
    hook never interrupts Claude Code."""
    txt = _read("sonara-hook.cmd")
    low = txt.lower()
    assert "pythonw" in low                       # preferred windowless interpreter
    assert "pyw -3" in low                         # fallback when pythonw is absent
    assert "hook.log" in low                       # stderr is logged...
    assert "2>>" in low                            # ...via append-redirect, not discarded
    assert "exit /b 0" in low                      # a hook must never fail loudly
