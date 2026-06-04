"""Run every captured golden payload through handle_event and assert the
returned messages are well-typed protocol dicts. Fixture file names are
'<Event>.json' or '<Event>-<pid>.json'; the leading token is the event."""
import json
from pathlib import Path

import pytest

from sonari.hooks_entry import handle_event
from sonari.protocol import PROTOCOL_VERSION, MsgType


FIXTURE_DIR = Path(__file__).parent / "fixtures"

_VALID_TYPES = {
    v for k, v in vars(MsgType).items()
    if not k.startswith("_") and isinstance(v, str)
}


def _event_name(path: Path) -> str:
    # 'MessageDisplay-12345.json' -> 'MessageDisplay'; 'Stop.json' -> 'Stop'
    return path.stem.split("-", 1)[0]


def _fixture_files():
    if not FIXTURE_DIR.is_dir():
        return []
    return sorted(p for p in FIXTURE_DIR.glob("*.json") if p.is_file())


@pytest.mark.parametrize(
    "fixture",
    _fixture_files(),
    ids=lambda p: p.name,
)
def test_fixture_parses_to_well_typed_messages(fixture):
    raw = fixture.read_text(encoding="utf-8")
    payload = json.loads(raw) if raw.strip() else {}
    assert isinstance(payload, dict), f"{fixture.name}: payload must be an object"

    event = _event_name(fixture)
    msgs = handle_event(event, payload)

    assert isinstance(msgs, list), f"{fixture.name}: handle_event must return a list"
    for msg in msgs:
        assert isinstance(msg, dict), f"{fixture.name}: each message must be a dict"
        assert msg.get("v") == PROTOCOL_VERSION, f"{fixture.name}: missing/bad protocol version"
        assert msg.get("type") in _VALID_TYPES, (
            f"{fixture.name}: bad message type {msg.get('type')!r}"
        )


def test_at_least_one_fixture_exists():
    """The capture task must have produced golden payloads; a green run with
    zero fixtures would be a silent false positive."""
    files = _fixture_files()
    assert files, (
        "no tests/fixtures/*.json captured; run the SONARI_CAPTURE capture task "
        "against a real session first"
    )
