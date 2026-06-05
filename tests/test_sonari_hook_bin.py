import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
HOOK = REPO / "bin" / "sonari-hook"


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
    res = _run("MessageDisplay", payload, {"SONARI_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "prose"
    assert lines[0]["delta"] == "Hi."


def test_hook_invalid_stdin_still_exits_zero(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    res = _run("MessageDisplay", b"not json at all", {"SONARI_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    # Empty/invalid stdin -> payload {} -> a prose message with empty delta is still produced.
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "prose"
    assert lines[0]["delta"] == ""


def test_hook_empty_stdin_exits_zero(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    res = _run("Stop", b"", {"SONARI_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "earcon"
    assert lines[0]["kind"] == "turn_done"


def test_hook_unknown_event_sends_nothing(tmp_path):
    sent_log = tmp_path / "sent.jsonl"
    res = _run("MadeUp", b"{}", {"SONARI_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    assert not sent_log.exists() or sent_log.read_text().strip() == ""


def test_hook_capture_dumps_raw_stdin(tmp_path):
    cap = tmp_path / "cap"
    cap.mkdir()
    sent_log = tmp_path / "sent.jsonl"
    raw = b'{"session_id": "s1", "delta": "Cap.", "index": 0, "final": true}'
    res = _run("MessageDisplay", raw, {"SONARI_CAPTURE": str(cap), "SONARI_FAKE_SENT_LOG": str(sent_log)})
    assert res.returncode == 0, res.stderr.decode()
    files = list(cap.glob("MessageDisplay-*.json"))
    assert len(files) == 1
    assert files[0].read_bytes() == raw


def test_hook_send_failure_is_swallowed(tmp_path):
    # When the fake client is told to raise, the shim must still exit 0.
    res = _run("Stop", b"{}", {"SONARI_FAKE_RAISE": "1"})
    assert res.returncode == 0, res.stderr.decode()


def test_hook_partial_batch_send_failure_does_not_drop_subsequent_messages(tmp_path):
    """A transient error on the first send of a two-message event must not
    prevent the second message from being attempted.

    PreToolUse + AskUserQuestion emits [EARCON(choice), CHOICE(...)].
    SONARI_FAKE_RAISE_ON=0 makes the fakeclient raise only on the first send
    (index 0) while the second send (index 1) succeeds and is logged.
    Without a per-send try/except, the exception from send[0] propagates out
    of main() and the second message is never attempted.
    The hook must still exit 0, and the second message must appear in the log.
    """
    sent_log = tmp_path / "sent.jsonl"
    payload = json.dumps({
        "session_id": "s1",
        "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"q": "Yes or no?", "options": ["Yes", "No"]}]},
    }).encode()
    res = _run(
        "PreToolUse",
        payload,
        {"SONARI_FAKE_SENT_LOG": str(sent_log), "SONARI_FAKE_RAISE_ON": "0"},
    )
    assert res.returncode == 0, res.stderr.decode()
    # The second message (CHOICE) must have been sent despite the error on the first.
    assert sent_log.exists(), "no messages logged — second send was not attempted"
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) >= 1, "expected at least the second message to be logged"
    assert lines[0]["type"] == "choice"


def test_hook_src_is_first_on_syspath_and_shadows_stale_global(tmp_path):
    """A stale globally-installed 'sonari' must NOT shadow the plugin's own src.

    We plant a fake 'sonari' package EARLIER on PYTHONPATH than the plugin src
    and assert the hook still resolves the real plugin package (its handle_event
    produces a real prose message). The rewritten shim inserts ../src at
    sys.path[0] before importing, so the real package wins.
    """
    stale = tmp_path / "stale"
    pkg = stale / "sonari"
    pkg.mkdir(parents=True)
    (pkg / "__init__.py").write_text("RAISE = True\n")
    # A stale hooks_entry that would crash if it were the one imported.
    (pkg / "hooks_entry.py").write_text("raise RuntimeError('stale wins')\n")

    sent_log = tmp_path / "sent.jsonl"
    env = dict(os.environ)
    # Put the STALE dir before the plugin src AND the fakeclient on PYTHONPATH.
    env["PYTHONPATH"] = os.pathsep.join([
        str(stale),
        str(REPO / "tests" / "_fakeclient"),
        str(REPO / "src"),
        env.get("PYTHONPATH", ""),
    ])
    env["SONARI_FAKE_SENT_LOG"] = str(sent_log)
    payload = json.dumps({"session_id": "s1", "delta": "Hi.",
                          "index": 0, "final": True}).encode()
    res = subprocess.run([sys.executable, str(HOOK), "MessageDisplay"],
                         input=payload, capture_output=True, env=env)
    assert res.returncode == 0, res.stderr.decode()
    lines = [json.loads(x) for x in sent_log.read_text().splitlines() if x.strip()]
    assert len(lines) == 1
    assert lines[0]["type"] == "prose"
    assert lines[0]["delta"] == "Hi."
