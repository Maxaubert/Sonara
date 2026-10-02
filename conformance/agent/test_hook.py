"""The Claude Code hook adapter (``sonara-hook.exe``, L5) end to end against
``sonarad``: hook events as Claude Code delivers them (event name in argv,
payload on stdin) become a session's turn in the runtime. Needs
``cargo build -p sonara-hook`` (or ``$SONARA_HOOK``)."""
from __future__ import annotations

import json
import time

import pytest

import harness

SESSION = "11111111-2222-3333-4444-555555555555"


@pytest.fixture(scope="module")
def hook_exe():
    exe = harness.find_hook()
    if exe is None or not exe.is_file():
        pytest.fail("sonara-hook.exe not found: run 'cargo build -p sonara-hook' or set SONARA_HOOK")
    return exe


def hook(exe, rt, event, payload, env=None):
    code = harness.Hook(exe, rt.home, event, env).send(payload)
    assert code == 0


def listener(rt):
    c = rt.tcp(extensions=["agent"])
    assert c.request({"type": "subscribe", "events": ["state", "earcons"]})["ok"]
    c.state()
    return c


def started(c, text, timeout=harness.TIMEOUT):
    """Wait until an item starts whose first chunk is ``text``."""
    return c.next_event(
        lambda e: e.get("event") == "state"
        and e["now_playing"] is not None
        and e["now_playing"]["text"] == text,
        timeout,
    )


def test_a_prompt_and_its_answer_are_read_in_the_sessions_channel(hook_exe, rt):
    c = listener(rt)
    hook(hook_exe, rt, "UserPromptSubmit", {"session_id": SESSION, "cwd": r"C:\work\proj"},
         {"SONARA_HOST_TAB": "tab-9"})
    hook(hook_exe, rt, "MessageDisplay",
         {"session_id": SESSION, "delta": "Here is the answer. ", "index": 0, "final": False})
    s = started(c, "Here is the answer.")
    assert s["now_playing"]["channel"] == SESSION
    assert s["now_playing"]["host_tab"] == "tab-9"
    hook(hook_exe, rt, "Stop", {"session_id": SESSION})
    assert c.next_event(lambda e: e.get("event") == "earcon")["kind"] == "turn_done"


def test_the_old_turns_late_text_is_dropped(hook_exe, rt):
    # #174 end to end: the old turn's MessageDisplay hook starts before the
    # new prompt's hook but delivers after it.
    c = listener(rt)
    hook(hook_exe, rt, "UserPromptSubmit", {"session_id": SESSION, "cwd": "/w/proj"})
    late = harness.Hook(hook_exe, rt.home, "MessageDisplay")
    time.sleep(0.05)
    hook(hook_exe, rt, "UserPromptSubmit", {"session_id": SESSION, "cwd": "/w/proj"})
    assert late.send({"session_id": SESSION, "delta": "Old turn tail.", "index": 3, "final": True}) == 0
    hook(hook_exe, rt, "MessageDisplay",
         {"session_id": SESSION, "delta": "New answer.", "index": 0, "final": True})
    s = started(c, "New answer.")
    assert s["now_playing"]["channel"] == SESSION
    for e in c.events:
        if e.get("event") == "state" and e["now_playing"] is not None:
            assert e["now_playing"]["text"] != "Old turn tail."


def test_a_question_chimes_and_its_permission_prompt_is_suppressed(hook_exe, rt):
    c = listener(rt)
    payload = harness.REPO / "tests" / "fixtures" / "PreToolUse-AskUserQuestion.json"
    hook(hook_exe, rt, "PreToolUse", json.loads(payload.read_text(encoding="utf-8")))
    assert c.next_event(lambda e: e.get("event") == "earcon")["kind"] == "choice"
    started(c, "Which color do you prefer?")
    hook(hook_exe, rt, "Notification",
         {"session_id": SESSION, "notification_type": "permission_prompt",
          "message": "Claude needs your permission to use AskUserQuestion"})
    with pytest.raises(AssertionError):
        c.next_event(lambda e: e.get("event") == "earcon", 0.5)
    hook(hook_exe, rt, "PostToolUse", {"session_id": SESSION, "tool_name": "AskUserQuestion"})
    c.state(lambda s: s["now_playing"] is None)


def test_a_session_end_closes_the_channel(hook_exe, rt):
    c = listener(rt)
    hook(hook_exe, rt, "SessionStart", {"session_id": SESSION, "cwd": "/w/proj"})
    hook(hook_exe, rt, "SessionEnd", {"session_id": SESSION})
    r = c.request({"type": "focus", "channel": SESSION})
    assert r["error"]["code"] == "E_NOT_FOUND"
