"""Test double for sonara.client: records sent messages instead of using a socket.

Loaded by tests/_fakeclient/sitecustomize.py at interpreter startup and registered
as sys.modules["sonara.client"] so bin/sonara-hook's `from sonara import client`
returns THIS client. The shim puts src/ at sys.path[0] unconditionally, so sonara
itself plus sonara.hooks_entry and sonara.protocol still resolve from src/; only
the pre-injected `client` submodule is overridden.

Environment variables:
  SONARA_FAKE_RAISE         -- raise on every call to ensure_daemon and send.
  SONARA_FAKE_RAISE_AFTER   -- integer N; raise on send calls with index >= N
                               (0-indexed), but let earlier calls succeed.
  SONARA_FAKE_RAISE_ON      -- integer N; raise only on send call with exact
                               index N (0-indexed); all other calls succeed.
  SONARA_FAKE_SENT_LOG      -- path to append sent messages as newline-delimited JSON.
  SONARA_FAKE_BATCH_RAISE   -- send_many raises DaemonNotRunning (nothing was
                               sent, so the hook falls back to one send per
                               message).
  SONARA_FAKE_BATCH_PARTIAL -- send_many logs the FIRST message, then raises
                               OSError: a partial write the hook must not resend.
  SONARA_FAKE_NO_BATCH      -- no send_many at all (an older client module).
  SONARA_FAKE_BATCH_LOG     -- path to append one JSON list of message types
                               per send_many call.
"""
import json
import os

# Module-level call counter so the subprocess-level state resets for each run.
_send_call_count = 0


class DaemonNotRunning(OSError):
    pass


def ensure_daemon(timeout: float = 3.0) -> None:
    if os.environ.get("SONARA_FAKE_RAISE"):
        raise RuntimeError("forced ensure_daemon failure")


def send(msg: dict, expect_reply: bool = False, timeout: float = 2.0):
    global _send_call_count
    call_index = _send_call_count
    _send_call_count += 1

    if os.environ.get("SONARA_FAKE_RAISE"):
        raise RuntimeError("forced send failure")

    raise_after_env = os.environ.get("SONARA_FAKE_RAISE_AFTER")
    if raise_after_env is not None:
        try:
            threshold = int(raise_after_env)
        except ValueError:
            threshold = 0
        if call_index >= threshold:
            raise RuntimeError(f"forced send failure at call index {call_index}")

    raise_on_env = os.environ.get("SONARA_FAKE_RAISE_ON")
    if raise_on_env is not None:
        try:
            target = int(raise_on_env)
        except ValueError:
            target = 0
        if call_index == target:
            raise RuntimeError(f"forced send failure at call index {call_index}")

    log = os.environ.get("SONARA_FAKE_SENT_LOG")
    if log:
        with open(log, "a") as f:
            f.write(json.dumps(msg) + "\n")
    return None


def send_many(msgs, timeout: float = 2.0) -> None:
    if os.environ.get("SONARA_FAKE_RAISE"):
        raise RuntimeError("forced send_many failure")
    if os.environ.get("SONARA_FAKE_BATCH_RAISE"):
        raise DaemonNotRunning("forced send_many failure")
    if os.environ.get("SONARA_FAKE_BATCH_PARTIAL"):
        log = os.environ.get("SONARA_FAKE_SENT_LOG")
        if log and msgs:
            with open(log, "a") as f:
                f.write(json.dumps(msgs[0]) + "\n")
        raise OSError("forced partial send_many write")
    batch_log = os.environ.get("SONARA_FAKE_BATCH_LOG")
    if batch_log:
        with open(batch_log, "a") as f:
            f.write(json.dumps([m.get("type") for m in msgs]) + "\n")
    log = os.environ.get("SONARA_FAKE_SENT_LOG")
    if log:
        with open(log, "a") as f:
            for m in msgs:
                f.write(json.dumps(m) + "\n")


if os.environ.get("SONARA_FAKE_NO_BATCH"):
    del send_many
