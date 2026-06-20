"""macOS earcon backend — wraps `afplay` + the System Sounds defaults."""
from __future__ import annotations

import os
import subprocess

from sonara.platform.base import EarconBackend

_DEFAULTS = {
    "permission": "/System/Library/Sounds/Funk.aiff",
    "choice":     "/System/Library/Sounds/Ping.aiff",
    "plan":       "/System/Library/Sounds/Submarine.aiff",
    "error":      "/System/Library/Sounds/Sosumi.aiff",
    "turn_done":  "/System/Library/Sounds/Tink.aiff",
    "ready":      "/System/Library/Sounds/Glass.aiff",
}


class MacEarconBackend(EarconBackend):
    def play(self, path: str):
        if not os.path.exists(path):
            return None
        try:
            return subprocess.Popen(["afplay", path])
        except (FileNotFoundError, OSError):
            return None

    def default_earcons(self) -> dict:
        return dict(_DEFAULTS)
