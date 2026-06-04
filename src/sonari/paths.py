import os
import socket
from pathlib import Path

SONARI_DIR = Path.home() / ".sonari"
CONFIG_PATH = SONARI_DIR / "config.json"
SOCKET_PATH = SONARI_DIR / "speechd.sock"
LOG_PATH = SONARI_DIR / "speechd.log"


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
