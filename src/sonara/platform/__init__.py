"""get_platform() -- the single OS dispatch point for Sonara."""
from __future__ import annotations

import sys

from sonara.platform.base import PlatformBackend

_CACHE = None


def _require_windows() -> None:
    if sys.platform != "win32":
        raise RuntimeError("Sonara is Windows-only")


def get_platform() -> PlatformBackend:
    global _CACHE
    if _CACHE is not None:
        return _CACHE
    _require_windows()
    from sonara.platform.windows import make_backend
    _CACHE = make_backend()
    return _CACHE


def daemon_process():
    """The OS module that prepares the daemon process: arm_faulthandler,
    harden_process, preload_vc_runtime and the single-instance guard
    (acquire_singleton_mutex, acquire_singleton). Light on purpose: it loads
    no speech, audio or hotkey backend, so daemon main() can use it before
    the VC runtime preload and get_platform() (#29)."""
    _require_windows()
    from sonara.platform.windows import process
    return process
