import json

from sonari import protocol
from sonari.protocol import MsgType, PROTOCOL_VERSION, encode, decode


def test_protocol_version_is_one():
    assert PROTOCOL_VERSION == 1


def test_encode_returns_bytes_ending_in_newline():
    msg = {"v": PROTOCOL_VERSION, "type": MsgType.PING}
    out = encode(msg)
    assert isinstance(out, bytes)
    assert out.endswith(b"\n")
    assert out.decode("utf-8") == json.dumps(msg) + "\n"


def test_decode_reverses_encode():
    msg = {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": "abc-123"}
    assert decode(encode(msg)) == msg


def test_round_trip_preserves_nested_and_unicode():
    msg = {
        "v": PROTOCOL_VERSION,
        "type": MsgType.CHOICE,
        "session": "s1",
        "questions": [{"q": "Pick one — café or tea?", "options": ["a", "b"]}],
        "n": 7,
        "flag": True,
        "empty": None,
    }
    line = encode(msg)
    assert isinstance(line, bytes)
    assert line.count(b"\n") == 1
    assert line.endswith(b"\n")
    assert decode(line) == msg


def test_decode_accepts_line_without_trailing_newline():
    # decode must tolerate a json.loads-able line whether or not it carries the delimiter
    msg = {"v": PROTOCOL_VERSION, "type": MsgType.STATUS}
    assert decode(b'{"v": 1, "type": "status"}') == msg


def test_encode_is_pure_module_function():
    assert callable(protocol.encode)
    assert callable(protocol.decode)


def test_msgtype_has_every_constant_with_exact_values():
    expected = {
        "PROSE": "prose",
        "CHOICE": "choice",
        "PLAN": "plan",
        "TOOL": "tool_announce",
        "PERMISSION": "permission",
        "EARCON": "earcon",
        "FLUSH": "flush",
        "SESSION_START": "session_start",
        "SESSION_END": "session_end",
        "SET_FOREGROUND": "set_foreground",
        "STOP": "stop",
        "SKIP": "skip",
        "REPEAT": "repeat",
        "JUMP_DECISION": "jump_decision",
        "CATCH_UP": "catch_up",
        "SET_RATE": "set_rate",
        "SET_VERBOSITY": "set_verbosity",
        "SET_VOICE": "set_voice",
        "STATUS": "status",
        "PING": "ping",
    }
    for name, value in expected.items():
        assert hasattr(MsgType, name), f"MsgType missing {name}"
        assert getattr(MsgType, name) == value, f"MsgType.{name} != {value!r}"


def test_msgtype_defines_no_extra_string_constants():
    actual = {
        k: v
        for k, v in vars(MsgType).items()
        if not k.startswith("_") and isinstance(v, str)
    }
    expected = {
        "PROSE": "prose",
        "CHOICE": "choice",
        "PLAN": "plan",
        "TOOL": "tool_announce",
        "PERMISSION": "permission",
        "EARCON": "earcon",
        "FLUSH": "flush",
        "SESSION_START": "session_start",
        "SESSION_END": "session_end",
        "SET_FOREGROUND": "set_foreground",
        "STOP": "stop",
        "SKIP": "skip",
        "REPEAT": "repeat",
        "JUMP_DECISION": "jump_decision",
        "CATCH_UP": "catch_up",
        "SET_RATE": "set_rate",
        "SET_VERBOSITY": "set_verbosity",
        "SET_VOICE": "set_voice",
        "STATUS": "status",
        "PING": "ping",
    }
    assert actual == expected


def test_msgtype_values_are_unique():
    values = [
        v for k, v in vars(MsgType).items()
        if not k.startswith("_") and isinstance(v, str)
    ]
    assert len(values) == len(set(values))
