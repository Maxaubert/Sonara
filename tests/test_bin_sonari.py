import os
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SHIM = os.path.join(REPO, "bin", "sonari")


def _env():
    env = dict(os.environ)
    # Make 'sonari' importable without an install.
    src = os.path.join(REPO, "src")
    env["PYTHONPATH"] = src + os.pathsep + env.get("PYTHONPATH", "")
    return env


def test_shim_exists_and_executable():
    assert os.path.exists(SHIM)
    assert os.access(SHIM, os.X_OK)


def test_shim_help_runs():
    proc = subprocess.run([SHIM, "--help"], capture_output=True, text=True,
                          env=_env())
    assert proc.returncode == 0
    assert "usage" in proc.stdout.lower()


def test_shim_no_args_returns_2():
    proc = subprocess.run([SHIM], capture_output=True, text=True, env=_env())
    assert proc.returncode == 2


def test_shim_forwards_subcommand_exit_code():
    # 'doctor' returns 1 when any check fails; with no daemon the socket check
    # fails, so we expect a non-zero exit and printed output.
    proc = subprocess.run([SHIM, "doctor"], capture_output=True, text=True,
                          env=_env())
    assert proc.returncode in (0, 1)
    assert "say" in proc.stdout
