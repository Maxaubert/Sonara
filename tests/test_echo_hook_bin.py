import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
HOOK = REPO / "bin" / "echo-hook"


def _run(event, stdin_bytes, extra_env=None):
    env = dict(os.environ)
    # Force the daemon/send path into a no-op fake so the shim never touches a socket.
    env["PYTHONPATH"] = str(REPO / "tests" / "_fakeclient") + os.pathsep + str(REPO / "src") + os.pathsep + env.get("PYTHONPATH", "")
    if extra_env:
        env.update(extra_env)
    return subprocess.run(
        [sys.executable, str(HOOK), event],
        input=stdin_bytes,
        capture_output=True,
        env=env,
    )


def test_hook_exists_and_is_executable():
    assert HOOK.exists(), f"missing {HOOK}"
    assert os.access(HOOK, os.X_OK), f"{HOOK} not executable"


def test_hook_sends_messages_and_exits_zero(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    payload = json.dumps({"session_id": "s1", "delta": "Hi.", "index": 0, "final": True}).encode()
    res = _run("MessageDisplay", payload, {"ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "prose"
    assert lines[0]["delta"] == "Hi."


def test_hook_invalid_stdin_still_exits_zero(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    res = _run("MessageDisplay", b"not json at all", {"ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    # Empty/invalid stdin -> payload {} -> a prose message with empty delta is still produced.
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "prose"
    assert lines[0]["delta"] == ""


def test_hook_empty_stdin_exits_zero(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    res = _run("Stop", b"", {"ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "earcon"
    assert lines[0]["kind"] == "turn_done"


def test_hook_unknown_event_sends_nothing(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    res = _run("MadeUp", b"{}", {"ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    assert not sent_log.exists() or sent_log.read_text().strip() == ""


def test_hook_capture_dumps_raw_stdin(tmp_path):
    cap = tmp_path / "cap"
    cap.mkdir()
    sent_log = tmp_path / "sent.jsonl"
    raw = b'{"session_id": "s1", "delta": "Cap.", "index": 0, "final": true}'
    res = _run("MessageDisplay", raw, {"ECHO_CAPTURE": str(cap), "ECHO_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    files = list(cap.glob("MessageDisplay-*.json"))
    assert len(files) == 1
    assert files[0].read_bytes() == raw


def test_hook_send_failure_is_swallowed(tmp_path):
    # When the fake client is told to raise, the shim must still exit 0.
    res = _run("Stop", b"{}", {"ECHO_FAKE_RAISE": "1"})
    assert res.returncode == 0, res.stderr.decode()
