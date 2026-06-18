"""Provision + wire the opt-in Kokoro neural-voice environment.

Kokoro needs Python >=3.10 (kokoro-onnx requires onnxruntime>=1.20.1 + numpy>=2),
but the daemon defaults to system /usr/bin/python3 (3.9). This module provisions a
uv-managed venv at paths.KOKORO_VENV and the daemon is repointed at it. "Neural
enabled" is derived from the venv's existence — no separate flag to drift.

All subprocess work goes through an injected ``run`` callable so the logic is
unit-testable without touching uv, the network, or a real venv.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys

from sonari import paths


def neural_enabled() -> bool:
    """True if the neural venv has been provisioned (its Python exists)."""
    return os.path.exists(paths.kokoro_venv_python())


# ---------------------------------------------------------------------------
# Task 3: ensure_uv
# ---------------------------------------------------------------------------

def _default_user_base(py: str) -> str:
    return subprocess.check_output(
        [py, "-c", "import site; print(site.getuserbase())"],
        text=True).strip()


def ensure_uv(which=shutil.which, run=subprocess.check_call,
              base_python=None, user_base=_default_user_base) -> str:
    """Return a path to `uv`, bootstrapping it via `pip install --user uv` when
    it is not already on PATH. Raises RuntimeError (actionable) if uv cannot be
    obtained — never returns a non-existent path."""
    found = which("uv")
    if found:
        return found
    py = base_python or sys.executable
    run([py, "-m", "pip", "install", "--user", "--quiet", "uv"])
    cand = os.path.join(user_base(py), "bin", "uv")
    if os.path.exists(cand):
        return cand
    found = which("uv")
    if found:
        return found
    raise RuntimeError(
        "Could not install or locate `uv`, needed to provision neural voices. "
        "Install uv (https://docs.astral.sh/uv/) and re-run: sonari voices install")


# ---------------------------------------------------------------------------
# Task 4: requirements_path + provision
# ---------------------------------------------------------------------------

def requirements_path() -> str:
    """Absolute path to the bundled pinned Kokoro requirements file."""
    return os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "requirements-kokoro.txt")


def provision(uv: str, run=subprocess.check_call) -> None:
    """Create the uv-managed venv (downloading CPython 3.12 if absent) and install
    the pinned Kokoro stack into it. Raises subprocess.CalledProcessError on failure
    (the caller aborts without rewiring the daemon)."""
    venv_dir = str(paths.KOKORO_VENV)
    run([uv, "venv", venv_dir, "--python", "3.12"])
    run([uv, "pip", "install", "--python", paths.kokoro_venv_python(),
         "-r", requirements_path()])


# ---------------------------------------------------------------------------
# Task 5: predownload_model + neural_healthy
# ---------------------------------------------------------------------------

_PREDOWNLOAD = (
    "from sonari import kokoro, paths as p; "
    "kokoro.KokoroEngine(p.SONARI_DIR / 'kokoro')._ensure_loaded()")

_HEALTH = "from sonari import kokoro; print(kokoro.is_installed())"


def predownload_model(app_dir: str, run=subprocess.check_call) -> None:
    """Trigger the one-time ~316 MB model download via the venv python, so the
    first real utterance does not stall for minutes."""
    env = dict(os.environ, PYTHONPATH=app_dir)
    run([paths.kokoro_venv_python(), "-c", _PREDOWNLOAD], env=env)


def neural_healthy(app_dir: str, run=subprocess.check_output) -> bool:
    """True if the venv python can import the Kokoro extra (kokoro.is_installed())."""
    env = dict(os.environ, PYTHONPATH=app_dir)
    try:
        out = run([paths.kokoro_venv_python(), "-c", _HEALTH], env=env, text=True)
    except Exception:  # noqa: BLE001 - any failure means "not healthy"
        return False
    return out.strip() == "True"
