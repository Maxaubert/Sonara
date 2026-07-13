"""Provision the opt-in Chatterbox GPU voice environment.

Chatterbox needs torch (Python <= 3.12; cu128 wheels for Blackwell GPUs like
the RTX 5090), which the system Python 3.14 cannot run. A uv-managed venv at
paths.CHATTERBOX_VENV holds the stack; "provisioned" is derived from the venv
python's existence (see chatterbox.is_provisioned()). All subprocess work
goes through injected callables so the logic is unit-testable (mirrors
kokoro_provision).

Deviation from the original design brief, per the real-GPU smoke test
(docs/superpowers/specs/2026-07-12-chatterbox-smoke.md): installing
chatterbox-tts SILENTLY DOWNGRADES a cu128 torch/torchaudio install to
generic CPU builds (the import still succeeds; only
`torch.cuda.is_available()` reveals the downgrade). The verified working
order is torch cu128, then chatterbox-tts, then torch cu128 --reinstall,
then torchaudio cu128 --reinstall - torchaudio must be reinstalled too,
because the leftover CPU torchaudio wheel's native extension is ABI-
incompatible with the reinstalled torch build. provision() reproduces that
exact four-step sequence.
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
    """Create the uv-managed venv (downloading CPython 3.12 if absent), then run
    the four-step install sequence verified in
    docs/superpowers/specs/2026-07-12-chatterbox-smoke.md: torch cu128, then
    chatterbox-tts (which downgrades torch/torchaudio to CPU builds), then
    torch cu128 --reinstall, then torchaudio cu128 --reinstall to undo the
    downgrade and restore ABI compatibility between the two. Raises
    subprocess.CalledProcessError on failure (the caller reverts)."""
    venv_dir = str(paths.CHATTERBOX_VENV)
    py = paths.chatterbox_venv_python()
    run([uv, "venv", venv_dir, "--python", "3.12"])
    # 1. torch cu128 first (Blackwell/RTX 5090 needs cu128).
    run([uv, "pip", "install", "--python", py, "torch",
         "--index-url", _TORCH_INDEX])
    # 2. chatterbox-tts. This SILENTLY DOWNGRADES torch/torchaudio to
    #    generic CPU builds; steps 3-4 undo that.
    run([uv, "pip", "install", "--python", py,
         "-r", chatterbox_requirements_path()])
    # 3. Force-reinstall torch cu128 to undo the downgrade from step 2.
    run([uv, "pip", "install", "--python", py, "torch",
         "--index-url", _TORCH_INDEX, "--reinstall"])
    # 4. torchaudio from step 2 is now ABI-incompatible with the reinstalled
    #    torch native extension; it must ALSO be reinstalled from cu128.
    run([uv, "pip", "install", "--python", py, "torchaudio",
         "--index-url", _TORCH_INDEX, "--reinstall"])


def warmup(run=subprocess.check_call) -> None:
    """Load the model once (downloads weights into HF_HOME) so the first real
    utterance does not stall for minutes. Runs a synth request through the
    worker's own handle_request, PYTHONPATH-ed to chatterbox_worker.py so the
    venv python can import it without sonara being on that venv.

    The worker loads the model on device="cuda" (chatterbox_worker.py), so a
    CPU-only torch/torchaudio install (see provision()'s docstring) would
    otherwise import fine and only silently run on CPU. The -c snippet checks
    torch.cuda.is_available() itself and asserts on False so that outcome
    fails provisioning loudly instead of succeeding on CPU."""
    from sonara.chatterbox import worker_script_path
    worker_dir = os.path.dirname(worker_script_path())
    env = dict(os.environ, HF_HOME=str(paths.CHATTERBOX_HF_CACHE),
               PYTHONPATH=worker_dir)
    code = ("import json, torch, chatterbox_worker as w; "
            "cuda_ok = torch.cuda.is_available(); "
            "print('cuda.is_available():', cuda_ok); "
            "assert cuda_ok, "
            "'chatterbox venv is CPU-only (torch.cuda.is_available() is "
            "False); torch/torchaudio must be reinstalled from the cu128 "
            "index'; "
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
