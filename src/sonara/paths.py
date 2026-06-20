from __future__ import annotations

import os
from pathlib import Path

SONARA_DIR = Path.home() / ".sonara"
APP_DIR = SONARA_DIR / "app"          # stable copy of the sonara package (PYTHONPATH target)
CONFIG_PATH = SONARA_DIR / "config.json"
LOCK_PATH = SONARA_DIR / "daemon.lock"
SINGLETON_PATH = SONARA_DIR / "daemon.singleton"   # held-open flock: single-instance
LOG_PATH = SONARA_DIR / "speechd.log"
KEYMAP_PATH = SONARA_DIR / "keymap.json"
HOTKEYD_RESOLVED_PATH = SONARA_DIR / "hotkeyd.resolved.json"
HOTKEYD_BIN_PATH = SONARA_DIR / "sonara-hotkeyd"
INSTALL_RECORD_PATH = SONARA_DIR / "install.json"
KOKORO_VENV = SONARA_DIR / "venv"   # opt-in uv-managed venv for neural voices


def kokoro_venv_python() -> str:
    """Absolute path to the neural venv's Python interpreter (may not exist)."""
    return str(KOKORO_VENV / "Scripts" / "python.exe")


def ensure_sonara_dir() -> None:
    SONARA_DIR.mkdir(parents=True, exist_ok=True)


def socket_connectable() -> bool:
    """Return True if the daemon is accepting connections (TCP lockfile)."""
    from sonara.platform import transport
    return transport.connectable(LOCK_PATH)


def repo_root() -> str:
    """Return the absolute path to the repository root.

    The canonical derivation: this file lives at <repo>/src/sonara/paths.py,
    so the repo root is two directories up from the directory containing it.
    """
    here = os.path.dirname(os.path.abspath(__file__))  # src/sonara
    return os.path.dirname(os.path.dirname(here))       # repo root
