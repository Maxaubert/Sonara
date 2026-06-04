import json
from pathlib import Path

from echo.hooks_entry import handle_event
from echo.protocol import PROTOCOL_VERSION, MsgType

FIXTURES = Path(__file__).parent / "fixtures"


def _load(name):
    return json.loads((FIXTURES / name).read_text())


def test_message_display_maps_to_prose():
    payload = {
        "session_id": "sess-1",
        "delta": "Hello there. How are you?",
        "index": 3,
        "final": False,
    }
    assert handle_event("MessageDisplay", payload) == [
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.PROSE,
            "session": "sess-1",
            "delta": "Hello there. How are you?",
            "index": 3,
            "final": False,
        }
    ]


def test_message_display_from_fixture():
    payload = _load("MessageDisplay.json")
    msgs = handle_event("MessageDisplay", payload)
    assert len(msgs) == 1
    m = msgs[0]
    assert m["type"] == MsgType.PROSE
    assert m["session"] == payload["session_id"]
    assert m["delta"] == payload["delta"]
    assert m["index"] == payload["index"]
    assert m["final"] == payload["final"]
    assert m["v"] == PROTOCOL_VERSION


def test_ask_user_question_earcon_then_choice():
    payload = {
        "session_id": "sess-1",
        "tool_name": "AskUserQuestion",
        "tool_input": {
            "questions": [
                {"question": "Pick one", "options": [{"label": "A"}, {"label": "B"}]}
            ]
        },
    }
    assert handle_event("PreToolUse", payload) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "choice"},
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.CHOICE,
            "session": "sess-1",
            "questions": [
                {"question": "Pick one", "options": [{"label": "A"}, {"label": "B"}]}
            ],
        },
    ]


def test_ask_user_question_from_fixture():
    payload = _load("PreToolUse-AskUserQuestion.json")
    msgs = handle_event("PreToolUse", payload)
    assert [m["type"] for m in msgs] == [MsgType.EARCON, MsgType.CHOICE]
    assert msgs[0]["kind"] == "choice"
    assert msgs[1]["session"] == payload["session_id"]
    assert msgs[1]["questions"] == payload["tool_input"]["questions"]
    assert all(m["v"] == PROTOCOL_VERSION for m in msgs)


def test_exit_plan_mode_earcon_then_plan():
    payload = {
        "session_id": "sess-1",
        "tool_name": "ExitPlanMode",
        "tool_input": {"plan": "Step one. Step two."},
    }
    assert handle_event("PreToolUse", payload) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "plan"},
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.PLAN,
            "session": "sess-1",
            "text": "Step one. Step two.",
        },
    ]


def test_exit_plan_mode_from_fixture():
    payload = _load("PreToolUse-ExitPlanMode.json")
    msgs = handle_event("PreToolUse", payload)
    assert [m["type"] for m in msgs] == [MsgType.EARCON, MsgType.PLAN]
    assert msgs[0]["kind"] == "plan"
    assert msgs[1]["session"] == payload["session_id"]
    assert msgs[1]["text"] == payload["tool_input"]["plan"]
    assert all(m["v"] == PROTOCOL_VERSION for m in msgs)


def test_pre_tool_use_bash_tool_announce():
    payload = {
        "session_id": "sess-1",
        "tool_name": "Bash",
        "tool_input": {"command": "git status", "description": "Show status"},
    }
    assert handle_event("PreToolUse", payload) == [
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.TOOL,
            "session": "sess-1",
            "tool": "Bash",
            "summary": "git status",
        }
    ]


def test_pre_tool_use_write_summary_is_basename():
    payload = {
        "session_id": "sess-1",
        "tool_name": "Write",
        "tool_input": {"file_path": "/Users/me/proj/src/echo/cli.py"},
    }
    msgs = handle_event("PreToolUse", payload)
    assert msgs == [
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.TOOL,
            "session": "sess-1",
            "tool": "Write",
            "summary": "cli.py",
        }
    ]


def test_pre_tool_use_edit_summary_is_basename():
    payload = {
        "session_id": "sess-1",
        "tool_name": "Edit",
        "tool_input": {"file_path": "/Users/me/proj/README.md"},
    }
    msgs = handle_event("PreToolUse", payload)
    assert msgs[0]["summary"] == "README.md"
    assert msgs[0]["tool"] == "Edit"


def test_pre_tool_use_unknown_tool_summary_is_tool_name():
    payload = {"session_id": "sess-1", "tool_name": "WebFetch", "tool_input": {}}
    msgs = handle_event("PreToolUse", payload)
    assert msgs == [
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.TOOL,
            "session": "sess-1",
            "tool": "WebFetch",
            "summary": "WebFetch",
        }
    ]


def test_pre_tool_use_bash_from_fixture():
    payload = _load("PreToolUse-Bash.json")
    msgs = handle_event("PreToolUse", payload)
    assert msgs[0]["type"] == MsgType.TOOL
    assert msgs[0]["tool"] == "Bash"
    assert msgs[0]["summary"] == "git status"
    assert msgs[0]["session"] == payload["session_id"]


def test_notification_permission_prompt():
    payload = {
        "session_id": "sess-1",
        "notification_type": "permission_prompt",
        "action": "Run git status",
    }
    assert handle_event("Notification", payload) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "permission"},
        {
            "v": PROTOCOL_VERSION,
            "type": MsgType.PERMISSION,
            "session": "sess-1",
            "action": "Run git status",
        },
    ]


def test_notification_permission_prompt_via_matcher_fallback():
    payload = {
        "session_id": "sess-1",
        "matcher": "permission_prompt",
        "action": "Edit file cli.py",
    }
    msgs = handle_event("Notification", payload)
    assert [m["type"] for m in msgs] == [MsgType.EARCON, MsgType.PERMISSION]
    assert msgs[0]["kind"] == "permission"
    assert msgs[1]["action"] == "Edit file cli.py"


def test_notification_idle_prompt():
    payload = {"session_id": "sess-1", "notification_type": "idle_prompt"}
    assert handle_event("Notification", payload) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "ready"}
    ]


def test_notification_permission_prompt_from_fixture():
    payload = _load("Notification-permission_prompt.json")
    msgs = handle_event("Notification", payload)
    assert [m["type"] for m in msgs] == [MsgType.EARCON, MsgType.PERMISSION]
    assert msgs[0]["kind"] == "permission"
    assert msgs[1]["session"] == payload["session_id"]
    assert msgs[1]["action"] == payload["action"]


def test_notification_idle_prompt_from_fixture():
    payload = _load("Notification-idle_prompt.json")
    msgs = handle_event("Notification", payload)
    assert msgs == [{"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "ready"}]


def test_unknown_notification_type_is_empty():
    payload = {"session_id": "sess-1", "notification_type": "something_else"}
    assert handle_event("Notification", payload) == []
