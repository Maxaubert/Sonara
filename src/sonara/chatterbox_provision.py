"""Provision the opt-in Chatterbox GPU voice environment.

Chatterbox needs torch (Python <= 3.12; cu128 wheels for Blackwell GPUs like
the RTX 5090), which the system Python 3.14 cannot run. A uv-managed venv at
paths.CHATTERBOX_VENV holds the stack; "provisioned" is derived from the venv
python's existence (see chatterbox.is_provisioned()). All subprocess work
goes through injected callables so the logic is unit-testable (mirrors
kokoro_provision).

Deviation from the original design brief, per the real-GPU smoke test
(docs/superpowers/specs/2026-07-12-chatterbox-smoke.md): a bare `torch`
install from the cu128 index is not enough. `chatterbox.tts` imports `perth`
at module load time, which imports `torchaudio.transforms` unconditionally,
so torchaudio must ALSO come from the cu128 index or the import fails with
an ABI mismatch (WinError 127 loading libtorchaudio.pyd). provision()
installs torch and torchaudio together from the cu128 index, then the pinned
requirements (which include chatterbox-tts).
"""
from __future__ import annotations

import os
import shutil
import subprocess

from sonara import paths
from sonara.kokoro_provision import ensure_uv

_TORCH_INDEX = "https://download.pytorch.org/whl/cu128"


def chatterbox_requirements_path() -> str:
    """Absolute path to the bundled pinned Chatterbox requirements file."""
    return os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "requirements-chatterbox.txt")


def provision(uv: str, run=subprocess.check_call) -> None:
    """Create the uv-managed venv (downloading CPython 3.12 if absent), install
    torch + torchaudio from the cu128 index, then the pinned Chatterbox stack.
    Raises subprocess.CalledProcessError on failure (the caller reverts)."""
    venv_dir = str(paths.CHATTERBOX_VENV)
    py = paths.chatterbox_venv_python()
    run([uv, "venv", venv_dir, "--python", "3.12"])
    run([uv, "pip", "install", "--python", py, "torch", "torchaudio",
         "--index-url", _TORCH_INDEX])
    run([uv, "pip", "install", "--python", py,
         "-r", chatterbox_requirements_path()])


def warmup(run=subprocess.check_call) -> None:
    """Load the model once (downloads weights into HF_HOME) so the first real
    utterance does not stall for minutes. Runs a synth request through the
    worker's own handle_request, PYTHONPATH-ed to chatterbox_worker.py so the
    venv python can import it without sonara being on that venv."""
    from sonara.chatterbox import worker_script_path
    worker_dir = os.path.dirname(worker_script_path())
    env = dict(os.environ, HF_HOME=str(paths.CHATTERBOX_HF_CACHE),
               PYTHONPATH=worker_dir)
    code = ("import json, chatterbox_worker as w; "
            "s = w.WorkerState(); "
            "print(json.dumps(w.handle_request(s, {'type': 'synth', "
            "'text': 'Chatterbox ready.', 'voice_path': None, "
            "'variant': 'turbo', 'exaggeration': None}))[:80])")
    run([paths.chatterbox_venv_python(), "-c", code], env=env)


def install_chatterbox(*, ensure_uv=ensure_uv, provision=provision,
                       warmup=warmup) -> None:
    """Provision the Chatterbox venv end-to-end. Any step raising aborts the
    whole operation (the caller reverts with uninstall_chatterbox)."""
    uv = ensure_uv()
    provision(uv)
    warmup()


def uninstall_chatterbox(rmtree=shutil.rmtree) -> None:
    """Remove the Chatterbox venv and its HF cache (idempotent)."""
    for p in (paths.CHATTERBOX_VENV, paths.CHATTERBOX_HF_CACHE):
        if os.path.isdir(str(p)):
            rmtree(str(p))
