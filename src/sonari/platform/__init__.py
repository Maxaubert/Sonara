"""get_platform() — the single OS dispatch point for Sonari."""
from __future__ import annotations

import sys

from sonari.platform.base import PlatformBackend

_CACHE = None


def get_platform() -> PlatformBackend:
    global _CACHE
    if _CACHE is not None:
        return _CACHE
    if sys.platform == "darwin":
        from sonari.platform.macos import make_backend
    elif sys.platform == "win32":
        raise RuntimeError("Windows backend lands in Milestone 2.")
    else:
        raise RuntimeError("Unsupported platform: {0}".format(sys.platform))
    _CACHE = make_backend()
    return _CACHE
