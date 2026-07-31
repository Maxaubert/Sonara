"""The two CLI launchers: bin/sonara (Git Bash) and bin/sonara.cmd (Windows).

Each platform exercises the launcher its users actually invoke. bin/sonara is a
bash script with a shebang, so on Windows CreateProcess cannot exec it directly
(OSError: [WinError 193]); it is driven through `bash` instead, when one exists.

Both launchers must FORWARD the CLI's exit code -- `sonara doctor` returns 1 on a
failed check and a bare `sonara` returns 2, and callers branch on that.
"""
import os
import shutil
import subprocess

import pytest

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASH_SHIM = os.path.join(REPO, "bin", "sonara")
CMD_SHIM = os.path.join(REPO, "bin", "sonara.cmd")

_WINDOWS = os.name == "nt"
NATIVE_SHIM = CMD_SHIM if _WINDOWS else BASH_SHIM

# A row doctor() always emits, on every platform (cli.doctor appends it inline).
# The old assertion looked for "say", which only ever appeared in the macOS rows.
_PORTABLE_DOCTOR_ROW = "SONARA_DIR writable"


def _env():
    """Make 'sonara' importable without an install.

    Deliberately sets PYTHONPATH to a NON-EMPTY value: that is the regression
    guard for the launchers' own PYTHONPATH join. bin/sonara used a POSIX ':'
    separator, which Windows python.exe cannot split, so a pre-set PYTHONPATH
    collapsed into one bogus entry and every call died with "No module named
    sonara". An empty PYTHONPATH happened to work, which is how it survived.
    """
    env = dict(os.environ)
    src = os.path.join(REPO, "src")
    existing = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = src + os.pathsep + existing if existing else src
    return env


def _run(argv):
    return subprocess.run(argv, capture_output=True, text=True,
                          env=_env(), cwd=REPO)


def _bash_argv(args):
    """Drive the bash shim through `bash`, with a POSIX-looking path so Git Bash
    does not mangle the backslashes."""
    return ["bash", BASH_SHIM.replace("\\", "/")] + list(args)


_needs_bash = pytest.mark.skipif(shutil.which("bash") is None,
                                 reason="no bash on PATH")


# ---------------------------------------------------------------------------
# presence
# ---------------------------------------------------------------------------

def test_both_shims_exist():
    assert os.path.exists(BASH_SHIM)
    assert os.path.exists(CMD_SHIM)


@pytest.mark.skipif(_WINDOWS,
                    reason="the executable bit is not meaningful on Windows; "
                           "os.access(X_OK) there is True for any file that exists")
def test_bash_shim_is_executable():
    assert os.access(BASH_SHIM, os.X_OK)


# ---------------------------------------------------------------------------
# the launcher this platform actually ships
# ---------------------------------------------------------------------------

def test_native_shim_help_runs():
    proc = _run([NATIVE_SHIM, "--help"])
    assert proc.returncode == 0, proc.stderr
    assert "usage" in proc.stdout.lower()


def test_native_shim_no_args_returns_2():
    # Regression: bin/sonara.cmd ended its python branch with a bare `exit /b`,
    # which returns 0 regardless of what the CLI exited with -- so a bare
    # `sonara` reported success instead of the argparse "no subcommand" 2.
    proc = _run([NATIVE_SHIM])
    assert proc.returncode == 2, (proc.returncode, proc.stdout, proc.stderr)


def test_native_shim_forwards_subcommand_exit_code():
    # 'doctor' exits 1 when any check fails and 0 when they all pass; which one
    # depends on whether a daemon is up, so accept either but require that the
    # rows were actually printed through the shim.
    proc = _run([NATIVE_SHIM, "doctor"])
    assert proc.returncode in (0, 1), (proc.returncode, proc.stderr)
    assert _PORTABLE_DOCTOR_ROW in proc.stdout, proc.stdout


# ---------------------------------------------------------------------------
# the bash shim, explicitly -- it stays the Git Bash entry point on Windows
# ---------------------------------------------------------------------------

@_needs_bash
def test_bash_shim_help_runs():
    proc = _run(_bash_argv(["--help"]))
    assert proc.returncode == 0, proc.stderr
    assert "usage" in proc.stdout.lower()


@_needs_bash
def test_bash_shim_no_args_returns_2():
    proc = _run(_bash_argv([]))
    assert proc.returncode == 2, (proc.returncode, proc.stdout, proc.stderr)


@_needs_bash
def test_bash_shim_survives_a_preset_pythonpath():
    """The separator regression, asserted on its own: _env() pre-sets PYTHONPATH,
    so an import failure here means the shim joined the paths with the wrong
    separator for this platform."""
    proc = _run(_bash_argv(["--help"]))
    assert "No module named" not in (proc.stderr or ""), proc.stderr
    assert proc.returncode == 0, proc.stderr
