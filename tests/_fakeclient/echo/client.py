"""Test double for echo.client: records sent messages instead of using a socket.

Shadowed onto PYTHONPATH ahead of src/ so bin/echo-hook imports THIS client.
The pure echo.hooks_entry and echo.protocol still resolve from src/ because this
package only provides a `client` submodule.
"""
import json
import os


def ensure_daemon(timeout: float = 3.0) -> None:
    if os.environ.get("ECHO_FAKE_RAISE"):
        raise RuntimeError("forced ensure_daemon failure")


def send(msg: dict, expect_reply: bool = False, timeout: float = 2.0):
    if os.environ.get("ECHO_FAKE_RAISE"):
        raise RuntimeError("forced send failure")
    log = os.environ.get("ECHO_FAKE_SENT_LOG")
    if log:
        with open(log, "a") as f:
            f.write(json.dumps(msg) + "\n")
    return None
