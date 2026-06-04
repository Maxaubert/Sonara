from pathlib import Path

ECHO_DIR = Path.home() / ".echo"
CONFIG_PATH = ECHO_DIR / "config.json"
SOCKET_PATH = ECHO_DIR / "speechd.sock"
LOG_PATH = ECHO_DIR / "speechd.log"


def ensure_echo_dir() -> None:
    ECHO_DIR.mkdir(parents=True, exist_ok=True)
