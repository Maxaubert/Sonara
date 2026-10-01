"""Interpreter and dependency provisioning: which Python the daemon runs on,
and getting the Windows speech engine (PyWinRT) into it."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
from typing import Optional

from sonara import paths
from sonara import platform as sonara_platform


def resolve_python():
    """Resolve the best Python >= 3.9 via the platform supervisor."""
    return sonara_platform.get_platform().supervisor.resolve_python()


def daemon_python(sup):
    """Interpreter the daemon should run on: the neural venv's Python when it is
    provisioned AND probes >=3.10, else the system Python from resolve_python().
    Deriving neural-state from the venv keeps re-runs of `sonara install` on the
    venv interpreter without a separate flag."""
    from sonara import kokoro_provision as kp
    return kp.usable_venv_python(sup._probe_python_version) or sup.resolve_python()


# The Windows speech engine (PyWinRT / OneCore). Kept in sync with the
# [windows] extra in pyproject.toml, requirements-kokoro.txt (the neural venv)
# and the hint in platform/windows/tts.py.
WINRT_PACKAGES = (
    "winrt-runtime",
    "winrt-Windows.Media.SpeechSynthesis",
    "winrt-Windows.Storage.Streams",
    "winrt-Windows.Media.Control",   # SMTC pause/resume of other apps' media (#92)
    "pycaw",     # per-app volume control for audio ducking
)


def winrt_importable(python: str) -> bool:
    """True if PyWinRT's OneCore speech projection imports under *python*."""
    try:
        r = subprocess.run(
            [python, "-c", "import winrt.windows.media.speechsynthesis"],
            capture_output=True, timeout=20)
        return r.returncode == 0
    except Exception:  # noqa: BLE001
        return False


def ensure_speech_deps(python: str) -> bool:
    """Make sure the Windows speech engine (PyWinRT) is installed in *python*.

    Speech needs the winrt-* packages, and Claude Code does NOT install a plugin's
    optional Python dependencies, so without this step a fresh install is silently
    voiceless. pip-installs them (idempotent: a no-op if already present), then
    verifies. Returns True iff speech can synthesize afterwards."""
    if winrt_importable(python):
        print("Speech engine (PyWinRT): already installed.")
        return True
    print("Installing the Windows speech engine (PyWinRT)...")
    console = console_sibling(python)
    cmd = speech_install_cmd(console, python_env(console), find_uv())
    try:
        subprocess.run(cmd, timeout=300)
    except Exception as exc:  # noqa: BLE001 - fall through to the verify + hint
        print(f"  the installer could not run: {exc}")
    if winrt_importable(python):
        print("Speech engine (PyWinRT): installed.")
        return True
    print("  Could not install PyWinRT automatically. Install it manually:\n    "
          + " ".join(cmd))
    return False


def console_sibling(python: str) -> str:
    """python.exe next to a pythonw.exe (installers and uv want the console
    interpreter); *python* itself otherwise."""
    head, tail = os.path.split(python)
    if tail.lower() == "pythonw.exe":
        cand = os.path.join(head, "python.exe")
        if os.path.isfile(cand):
            return cand
    return python


_PY_ENV_PROBE = (
    "import importlib.util, json, os, sys, sysconfig; print(json.dumps({"
    "'venv': sys.prefix != sys.base_prefix, "
    "'managed': os.path.isfile(os.path.join("
    "sysconfig.get_path('stdlib'), 'EXTERNALLY-MANAGED')), "
    "'pip': importlib.util.find_spec('pip') is not None}))")


def python_env(python: str) -> dict:
    """What kind of interpreter *python* is: {'venv', 'managed', 'pip'} as
    booleans ({} when the probe fails, which reads as a plain system Python).
    'managed' is a PEP 668 EXTERNALLY-MANAGED marker, as uv's own Pythons carry."""
    try:
        r = subprocess.run([python, "-c", _PY_ENV_PROBE], capture_output=True,
                           text=True, timeout=20)
        data = json.loads(r.stdout)
        return data if isinstance(data, dict) else {}
    except Exception:  # noqa: BLE001 - an unknown interpreter keeps the old path
        return {}


def find_uv() -> Optional[str]:
    """uv on PATH, else the copy the bootstrap downloaded to ~/.sonara/tools."""
    return shutil.which("uv") or local_uv()


def local_uv() -> Optional[str]:
    """The uv the bootstrap downloaded to ~/.sonara/tools (never on PATH)."""
    local = os.path.join(str(paths.SONARA_DIR), "tools", "uv.exe")
    return local if os.path.isfile(local) else None


def speech_install_cmd(python: str, env: dict, uv: Optional[str]) -> list:
    """The command that installs WINRT_PACKAGES into *python*.

    `pip install --user` only fits a plain system Python. A uv-managed Python
    is externally managed (PEP 668) and refuses it (E1); a venv rejects
    --user, and a uv venv has no pip at all (E2). uv installs into either."""
    pkgs = list(WINRT_PACKAGES)
    if env.get("venv"):
        if env.get("pip") or not uv:
            return [python, "-m", "pip", "install", *pkgs]
        return [uv, "pip", "install", "--python", python, *pkgs]
    if env.get("managed"):
        if uv:
            return [uv, "pip", "install", "--python", python,
                    "--break-system-packages", *pkgs]
        return [python, "-m", "pip", "install", "--user",
                "--break-system-packages", *pkgs]
    return [python, "-m", "pip", "install", "--user", *pkgs]
