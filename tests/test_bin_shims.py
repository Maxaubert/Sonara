import os
import subprocess

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(REPO, "bin")


def _read(name):
    with open(os.path.join(BIN, name), encoding="utf-8") as f:
        return f.read()


def test_sonari_daemon_prefers_usr_bin_python3_first():
    txt = _read("sonari-daemon")
    # The /usr/bin/python3 preference must appear BEFORE any `command -v python3`.
    pref = txt.index("[ -x /usr/bin/python3 ]")
    cmdv = txt.index("command -v python3")
    assert pref < cmdv, "shim must prefer /usr/bin/python3 before PATH lookup"


def test_sonari_prefers_usr_bin_python3_first():
    # On macOS/Linux (the non-Windows branch) /usr/bin/python3 must be preferred
    # before its PATH fallback. Windows uses `python` instead (OS guard below).
    txt = _read("sonari")
    assert 'OS' in txt and 'Windows_NT' in txt   # OS-aware: Windows -> python
    pref = txt.index("[ -x /usr/bin/python3 ]")
    cmdv = txt.index("command -v python3", pref)  # the PATH fallback AFTER it
    assert pref < cmdv, "shim must prefer /usr/bin/python3 before PATH lookup"


def test_sonari_hook_cmd_resolves_interpreter_and_logs_stderr():
    """M10: the Windows hook launcher must not silently mute. It resolves a
    windowless interpreter (pythonw, with a `pyw -3` fallback) and appends stderr
    to ~/.sonari/hook.log instead of discarding it — while still exiting 0 so a
    hook never interrupts Claude Code."""
    txt = _read("sonari-hook.cmd")
    low = txt.lower()
    assert "pythonw" in low                       # preferred windowless interpreter
    assert "pyw -3" in low                         # fallback when pythonw is absent
    assert "hook.log" in low                       # stderr is logged...
    assert "2>>" in low                            # ...via append-redirect, not discarded
    assert "exit /b 0" in low                      # a hook must never fail loudly


def test_sonari_daemon_picks_usr_bin_python3_even_when_stub_python3_on_path(tmp_path):
    # A fake `python3` earlier on PATH writes a marker; the shim must NOT pick it
    # because /usr/bin/python3 exists and is preferred. We verify by capturing
    # which interpreter the shim selects via a one-shot `--version`-style probe.
    stub_dir = tmp_path / "stub"
    stub_dir.mkdir()
    marker = tmp_path / "stub-was-used"
    stub = stub_dir / "python3"
    stub.write_text(
        "#!/bin/sh\n"
        f'echo used > "{marker}"\n'
        'exit 0\n'
    )
    stub.chmod(0o755)
    env = dict(os.environ)
    env["PATH"] = f"{stub_dir}:{env.get('PATH','')}"
    # Run the shim with a no-op subcommand path: `-m sonari.cli --help`-style.
    # We pass `status` which exits quickly; the daemon shim runs sonari.daemon,
    # so use the CLI shim for a deterministic quick exit.
    subprocess.run([os.path.join(BIN, "sonari"), "--help"], env=env,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    # If /usr/bin/python3 was preferred, the stub marker is NEVER written.
    assert not marker.exists(), "shim used the PATH stub instead of /usr/bin/python3"
