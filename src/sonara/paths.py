from __future__ import annotations

import os
from pathlib import Path

SONARA_DIR = Path.home() / ".sonara"
APP_DIR = SONARA_DIR / "app"          # stable copy of the sonara package (PYTHONPATH target)
CONFIG_PATH = SONARA_DIR / "config.json"
LOCK_PATH = SONARA_DIR / "daemon.lock"
# Stop sentinel (#23): existence means "Sonara must not run". Written by
# `sonara shutdown`, cleared by `sonara start` / install(). Gates BOTH respawn
# paths (the supervisor loop and the per-hook-event lazy start).
STOPPED_SENTINEL_PATH = SONARA_DIR / "stopped"
# Settings-page token, durable across daemon restarts (#34): the lockfile is
# unlinked on clean exit, so token reuse (page reconnect, stable bookmarks)
# needs its own small file.
WEBUI_TOKEN_PATH = SONARA_DIR / "webui.token"
SINGLETON_PATH = SONARA_DIR / "daemon.singleton"   # held-open flock: single-instance
LOG_PATH = SONARA_DIR / "speechd.log"
KEYMAP_PATH = SONARA_DIR / "keymap.json"
HOTKEYD_RESOLVED_PATH = SONARA_DIR / "hotkeyd.resolved.json"
HOTKEYD_BIN_PATH = SONARA_DIR / "sonara-hotkeyd"
INSTALL_RECORD_PATH = SONARA_DIR / "install.json"
SESSIONS_PATH = SONARA_DIR / "sessions.json"        # durable session id -> folder name map
KOKORO_VENV = SONARA_DIR / "venv"   # opt-in uv-managed venv for neural voices
PYTHON_RECORD_PATH = SONARA_DIR / "python.path"     # recorded console python.exe
PYTHONW_RECORD_PATH = SONARA_DIR / "pythonw.path"   # recorded windowless pythonw.exe


def _read_recorded(record: "Path") -> "str | None":
    """The interpreter path written in *record*, iff it still exists as a file."""
    try:
        path = record.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return path if path and Path(path).is_file() else None


def recorded_python() -> "str | None":
    """The console interpreter the bootstrap recorded (python.exe), or None."""
    return _read_recorded(PYTHON_RECORD_PATH)


def recorded_pythonw() -> "str | None":
    """The windowless interpreter the bootstrap recorded (pythonw.exe), or None."""
    return _read_recorded(PYTHONW_RECORD_PATH)


def kokoro_venv_python() -> str:
    """Absolute path to the neural venv's Python interpreter (may not exist)."""
    return str(KOKORO_VENV / "Scripts" / "python.exe")


CHATTERBOX_VENV = SONARA_DIR / "chatterbox-venv"    # opt-in uv venv for Chatterbox
CHATTERBOX_HF_CACHE = SONARA_DIR / "chatterbox" / "hf-cache"
CHATTERBOX_VOICES_DIR = SONARA_DIR / "voices" / "chatterbox"


def chatterbox_venv_python() -> str:
    """Absolute path to the Chatterbox venv's Python (may not exist)."""
    return str(CHATTERBOX_VENV / "Scripts" / "python.exe")


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
