"""Provision + wire the opt-in Kokoro neural-voice environment.

Kokoro needs Python >=3.10 (kokoro-onnx requires onnxruntime>=1.20.1 + numpy>=2),
which the system Python may not satisfy. This module provisions a uv-managed venv
at paths.KOKORO_VENV and the daemon is repointed at it. "Neural enabled" is derived
from the venv's existence -- no separate flag to drift.

All subprocess work goes through an injected ``run`` callable so the logic is
unit-testable without touching uv, the network, or a real venv.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys

from sonara import paths


def neural_enabled() -> bool:
    """True if the neural venv has been provisioned (its Python exists)."""
    return os.path.exists(paths.kokoro_venv_python())


def usable_venv_python(probe) -> "str | None":
    """The neural venv's python.exe when it is provisioned AND *probe* (a
    callable returning (major, minor) or None) reports >= 3.10, else None.
    The one venv choice behind both cli._daemon_python and the Windows
    daemon_pythonw (L-interp-dup)."""
    if not neural_enabled():
        return None
    venv_py = paths.kokoro_venv_python()
    ver = probe(venv_py)
    if ver is not None and ver >= (3, 10):
        return venv_py
    return None


# ---------------------------------------------------------------------------
# Task 3: ensure_uv
# ---------------------------------------------------------------------------

def _default_user_scripts(py: str) -> str:
    return subprocess.check_output(
        [py, "-c", "import sysconfig, os; print(sysconfig.get_path('scripts', os.name + '_user'))"],
        text=True).strip()


def _local_uv():
    from sonara.install import deps
    return deps.local_uv()


def _python_env(py: str) -> dict:
    from sonara.install import deps
    return deps.python_env(py)


_NO_UV = ("Could not install or locate `uv`, needed to provision neural voices. "
          "Install uv (https://docs.astral.sh/uv/) and re-run: sonara voices install")


def ensure_uv(which=shutil.which, run=subprocess.check_call,
              base_python=None, user_scripts=_default_user_scripts,
              local_uv=_local_uv, py_env=_python_env) -> str:
    """Return a path to `uv`: on PATH, else the copy the bootstrap downloaded
    to ~/.sonara/tools (E2-uv, the deps.find_uv order), else bootstrapped via
    `pip install --user uv`. A PEP 668 (externally managed) Python, such as
    the uv-managed one a zero-Python install runs on, refuses --user (E1), so
    it is never tried there. Raises RuntimeError (actionable) if uv cannot be
    obtained -- never returns a non-existent path."""
    found = which("uv") or local_uv()
    if found:
        return found
    py = base_python or sys.executable
    if py_env(py).get("managed"):
        raise RuntimeError(_NO_UV)
    run([py, "-m", "pip", "install", "--user", "--quiet", "uv"])
    cand = os.path.join(user_scripts(py), "uv.exe")
    if os.path.exists(cand):
        return cand
    found = which("uv")
    if found:
        return found
    raise RuntimeError(_NO_UV)


# ---------------------------------------------------------------------------
# Task 4: requirements_path + provision
# ---------------------------------------------------------------------------

def requirements_path() -> str:
    """Absolute path to the bundled pinned Kokoro requirements file."""
    return os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "requirements-kokoro.txt")


def _venv_python_starts(python: str) -> bool:
    """True if *python* runs at all. A venv whose base interpreter is gone
    keeps its python.exe stub, which then fails to start."""
    try:
        r = subprocess.run([python, "-c", "import sys"], capture_output=True,
                           timeout=30,
                           creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    except Exception:  # noqa: BLE001 - any failure means it cannot start
        return False
    return r.returncode == 0


def provision(uv: str, run=subprocess.check_call, starts=_venv_python_starts,
              rmtree=shutil.rmtree) -> None:
    """Create the uv-managed venv (downloading CPython 3.12 if absent) and install
    the pinned Kokoro stack into it. Raises subprocess.CalledProcessError on failure
    (the caller aborts without rewiring the daemon). An existing venv is
    reused and its packages upgraded in place: recreating it would first
    delete a working one (E10). One whose python cannot start (its base
    interpreter is gone) is rebuilt: reusing it could never succeed."""
    venv_dir = str(paths.KOKORO_VENV)
    venv_py = paths.kokoro_venv_python()
    if os.path.exists(venv_py) and not starts(venv_py):
        rmtree(venv_dir)
    if not os.path.exists(venv_py):
        run([uv, "venv", venv_dir, "--python", "3.12"])
    run([uv, "pip", "install", "--python", paths.kokoro_venv_python(),
         "-r", requirements_path()])


# ---------------------------------------------------------------------------
# Task 5: predownload_model + neural_healthy
# ---------------------------------------------------------------------------

# force=True: an explicit install retries even inside the cool-down a failed
# background download left behind (E11).
_PREDOWNLOAD = (
    "from sonara import kokoro, paths as p; "
    "e = kokoro.KokoroEngine(p.SONARA_DIR / 'kokoro'); "
    "e.download_models(force=True); e._ensure_loaded()")

_HEALTH = "from sonara import kokoro; print(kokoro.is_installed())"


def predownload_model(pythonpath: str, run=subprocess.check_call) -> None:
    """Trigger the one-time ~316 MB model download via the venv python, so the
    first real utterance does not stall for minutes."""
    env = dict(os.environ, PYTHONPATH=pythonpath)
    run([paths.kokoro_venv_python(), "-c", _PREDOWNLOAD], env=env)


_IMPORTABLE = ("import importlib.util as u; "
               "print(all(u.find_spec(m) is not None for m in ('numpy', 'kokoro_onnx')))")


def kokoro_importable(python: str, run=subprocess.check_output) -> bool:
    """True if *python* (the daemon's interpreter) can import the Kokoro
    extra. Kokoro may live in that Python's own site-packages, not in the
    neural venv; doctor must report what the daemon will really use."""
    try:
        out = run([python, "-c", _IMPORTABLE], text=True, timeout=20,
                  stderr=subprocess.DEVNULL,
                  creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    except Exception:  # noqa: BLE001 - any failure means "not importable"
        return False
    return out.strip() == "True"


def neural_healthy(app_dir: str, run=subprocess.check_output) -> bool:
    """True if the venv python can import the Kokoro extra (kokoro.is_installed())."""
    env = dict(os.environ, PYTHONPATH=app_dir)
    try:
        out = run([paths.kokoro_venv_python(), "-c", _HEALTH], env=env, text=True)
    except Exception:  # noqa: BLE001 - any failure means "not healthy"
        return False
    return out.strip() == "True"


# ---------------------------------------------------------------------------
# Task 6: install_kokoro + uninstall_kokoro orchestrators
# ---------------------------------------------------------------------------

def install_kokoro(pythonpath, *, ensure_uv=ensure_uv, provision=provision,
                   predownload_model=predownload_model) -> None:
    """Provision the neural venv end-to-end. Any step raising aborts the whole
    operation (the caller reports it and leaves the daemon on its current
    interpreter)."""
    uv = ensure_uv()
    provision(uv)
    predownload_model(pythonpath)


def uninstall_kokoro(rmtree=shutil.rmtree) -> None:
    """Remove the neural venv (idempotent). The daemon reverts to system Python on
    the next install/wiring because neural_enabled() then returns False."""
    if os.path.isdir(str(paths.KOKORO_VENV)):
        rmtree(str(paths.KOKORO_VENV))
