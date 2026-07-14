"""get_platform() -- the single OS dispatch point for Sonara."""
from __future__ import annotations

import sys

from sonara.platform.base import PlatformBackend

_CACHE = None


def get_platform() -> PlatformBackend:
    global _CACHE
    if _CACHE is not None:
        return _CACHE
    if sys.platform != "win32":
        raise RuntimeError("Sonara is Windows-only")
    from sonara.platform.windows import make_backend
    _CACHE = make_backend()
    return _CACHE
