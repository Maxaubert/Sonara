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
