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

from sonari import paths


def neural_enabled() -> bool:
    """True if the neural venv has been provisioned (its Python exists)."""
    return os.path.exists(paths.kokoro_venv_python())
