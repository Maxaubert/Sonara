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
