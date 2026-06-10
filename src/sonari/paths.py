from __future__ import annotations

import os
import socket
from pathlib import Path

SONARI_DIR = Path.home() / ".sonari"
APP_DIR = SONARI_DIR / "app"          # stable copy of the sonari package (PYTHONPATH target)
CONFIG_PATH = SONARI_DIR / "config.json"
SOCKET_PATH = SONARI_DIR / "speechd.sock"
LOG_PATH = SONARI_DIR / "speechd.log"
KEYMAP_PATH = SONARI_DIR / "keymap.json"
HOTKEYD_RESOLVED_PATH = SONARI_DIR / "hotkeyd.resolved.json"
HOTKEYD_BIN_PATH = SONARI_DIR / "sonari-hotkeyd"
INSTALL_RECORD_PATH = SONARI_DIR / "install.json"
PROMPT_OPEN_PATH = SONARI_DIR / "prompt-open"   # exists IFF a caret-trackable prompt is open


def ensure_sonari_dir() -> None:
    SONARI_DIR.mkdir(parents=True, exist_ok=True)


def socket_connectable() -> bool:
    """Return True if the daemon socket is accepting connections."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect(str(SOCKET_PATH))
        return True
    except OSError:
        return False
    finally:
        try:
            s.close()
        except OSError:
            pass


def repo_root() -> str:
    """Return the absolute path to the repository root.

    The canonical derivation: this file lives at <repo>/src/sonari/paths.py,
    so the repo root is two directories up from the directory containing it.
    """
    here = os.path.dirname(os.path.abspath(__file__))  # src/sonari
    return os.path.dirname(os.path.dirname(here))       # repo root
