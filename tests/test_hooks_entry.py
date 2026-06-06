import json
from pathlib import Path

from sonari.hooks_entry import handle_event
from sonari.protocol import PROTOCOL_VERSION, MsgType

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
        "tool_input": {"file_path": "/Users/me/proj/src/sonari/cli.py"},
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


def test_stop_emits_turn_done_earcon():
    assert handle_event("Stop", {"session_id": "sess-1"}) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "turn_done"}
    ]


def test_user_prompt_submit_sets_foreground_then_flush():
    assert handle_event("UserPromptSubmit", {"session_id": "sess-9"}) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "sess-9"},
        {"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "sess-9"},
    ]


def test_session_start_carries_plugin_version_and_root_from_env(monkeypatch):
    monkeypatch.setenv("CLAUDE_PLUGIN_VERSION", "0.4.0")
    monkeypatch.setenv("CLAUDE_PLUGIN_ROOT", "/plug/root")
    assert handle_event("SessionStart", {"session_id": "sess-9"}) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "sess-9"},
        {"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START, "session": "sess-9",
         "plugin_version": "0.4.0", "plugin_root": "/plug/root"},
    ]


def test_session_start_empty_strings_when_env_unset(monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    monkeypatch.delenv("CLAUDE_PLUGIN_ROOT", raising=False)
    msgs = handle_event("SessionStart", {"session_id": "sess-9"})
    assert msgs[1]["plugin_version"] == ""
    assert msgs[1]["plugin_root"] == ""


def test_session_end_emits_session_end():
    assert handle_event("SessionEnd", {"session_id": "sess-9"}) == [
        {"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END, "session": "sess-9"}
    ]


def test_unknown_event_is_empty():
    assert handle_event("TotallyMadeUp", {"session_id": "sess-1"}) == []


def test_missing_session_id_defaults_to_empty_string(monkeypatch):
    monkeypatch.delenv("CLAUDE_PLUGIN_VERSION", raising=False)
    monkeypatch.delenv("CLAUDE_PLUGIN_ROOT", raising=False)
    msgs = handle_event("SessionStart", {})
    assert msgs[0]["session"] == ""
    assert msgs[1]["session"] == ""
