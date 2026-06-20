"""get_platform() — the single OS dispatch point for Sonara."""
from __future__ import annotations

import sys

from sonara.platform.base import PlatformBackend

_CACHE = None


def get_platform() -> PlatformBackend:
    global _CACHE
    if _CACHE is not None:
        return _CACHE
    if sys.platform == "darwin":
        from sonara.platform.macos import make_backend
    elif sys.platform == "win32":
        from sonara.platform.windows import make_backend
    else:
        raise RuntimeError("Unsupported platform: {0}".format(sys.platform))
    _CACHE = make_backend()
    return _CACHE
