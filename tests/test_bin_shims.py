import os

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(REPO, "bin")


def _read(name):
    with open(os.path.join(BIN, name), encoding="utf-8") as f:
        return f.read()


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
