"""Test double for sonari.client: records sent messages instead of using a socket.

Loaded by tests/_fakeclient/sitecustomize.py at interpreter startup and registered
as sys.modules["sonari.client"] so bin/sonari-hook's `from sonari import client`
returns THIS client. The shim puts src/ at sys.path[0] unconditionally, so sonari
itself plus sonari.hooks_entry and sonari.protocol still resolve from src/; only
the pre-injected `client` submodule is overridden.

Environment variables:
  SONARI_FAKE_RAISE         -- raise on every call to ensure_daemon and send.
  SONARI_FAKE_RAISE_AFTER   -- integer N; raise on send calls with index >= N
                               (0-indexed), but let earlier calls succeed.
  SONARI_FAKE_RAISE_ON      -- integer N; raise only on send call with exact
                               index N (0-indexed); all other calls succeed.
  SONARI_FAKE_SENT_LOG      -- path to append sent messages as newline-delimited JSON.
"""
import json
import os

# Module-level call counter so the subprocess-level state resets for each run.
_send_call_count = 0


def ensure_daemon(timeout: float = 3.0) -> None:
    if os.environ.get("SONARI_FAKE_RAISE"):
        raise RuntimeError("forced ensure_daemon failure")


def send(msg: dict, expect_reply: bool = False, timeout: float = 2.0):
    global _send_call_count
    call_index = _send_call_count
    _send_call_count += 1

    if os.environ.get("SONARI_FAKE_RAISE"):
        raise RuntimeError("forced send failure")

    raise_after_env = os.environ.get("SONARI_FAKE_RAISE_AFTER")
    if raise_after_env is not None:
        try:
            threshold = int(raise_after_env)
        except ValueError:
            threshold = 0
        if call_index >= threshold:
            raise RuntimeError(f"forced send failure at call index {call_index}")

    raise_on_env = os.environ.get("SONARI_FAKE_RAISE_ON")
    if raise_on_env is not None:
        try:
            target = int(raise_on_env)
        except ValueError:
            target = 0
        if call_index == target:
            raise RuntimeError(f"forced send failure at call index {call_index}")

    log = os.environ.get("SONARI_FAKE_SENT_LOG")
    if log:
        with open(log, "a") as f:
            f.write(json.dumps(msg) + "\n")
    return None
