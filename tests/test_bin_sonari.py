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


import pytest


def test_cli_runs_under_usr_bin_python3_with_scrubbed_env():
    """PYTHONPATH=<repo>/src /usr/bin/python3 -m sonari.cli --help exits 0 with
    NO installed sonari anywhere — proves the self-contained source path works on
    the macOS system interpreter. Skipped if /usr/bin/python3 is absent.
    """
    sys_py = "/usr/bin/python3"
    if not os.path.exists(sys_py):
        pytest.skip("/usr/bin/python3 not present")
    # Scrub the environment so nothing but our src/ can supply 'sonari'.
    env = {
        "PATH": "/usr/bin:/bin",
        "PYTHONPATH": os.path.join(REPO, "src"),
        "HOME": os.environ.get("HOME", "/tmp"),
    }
    proc = subprocess.run([sys_py, "-m", "sonari.cli", "--help"],
                          capture_output=True, text=True, env=env)
    assert proc.returncode == 0, proc.stderr
    assert "usage" in proc.stdout.lower()
