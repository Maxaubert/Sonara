import os
import socket
from pathlib import Path

ECHO_DIR = Path.home() / ".echo"
CONFIG_PATH = ECHO_DIR / "config.json"
SOCKET_PATH = ECHO_DIR / "speechd.sock"
LOG_PATH = ECHO_DIR / "speechd.log"


def ensure_echo_dir() -> None:
    ECHO_DIR.mkdir(parents=True, exist_ok=True)


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

    The canonical derivation: this file lives at <repo>/src/echo/paths.py,
    so the repo root is two directories up from the directory containing it.
    """
    here = os.path.dirname(os.path.abspath(__file__))  # src/echo
    return os.path.dirname(os.path.dirname(here))       # repo root
