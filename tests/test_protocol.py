import json

from sonara import protocol
from sonara.protocol import MsgType, PROTOCOL_VERSION, encode, decode


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
        "questions": [{"q": "Pick one -- café or tea?", "options": ["a", "b"]}],
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
        "CHOICE_ANSWERED": "choice_answered",
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
        "NAV": "nav",
        "FLUSH_SESSION": "flush_session",
        "PAUSE": "pause",
        "MUTE": "mute",
        "NEXT_SESSION": "next_session",
        "REPEAT": "repeat",
        "JUMP_DECISION": "jump_decision",
        "CATCH_UP": "catch_up",
        "SET_RATE": "set_rate",
        "SET_VERBOSITY": "set_verbosity",
        "SET_VOICE": "set_voice",
        "SET_MINQUEUE": "set_minqueue",
        "SET_AUDIO_CONTROL": "set_audio_control",
        "SET_DUCK_LEVEL": "set_duck_level",
        "SET_VOLUME": "set_volume",
        "SET_SUMMARY_MODE": "set_summary_mode",
        "SET_AUDIO_MODE": "set_audio_mode",
        "SET_SESSION_PREF": "set_session_pref",
        "FORGET_SESSION": "forget_session",
        "SHUTDOWN": "shutdown",
        "STATUS": "status",
        "PING": "ping",
        "REREAD_OPTIONS": "reread_options",
        "CYCLE_VERBOSITY": "cycle_verbosity",
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
        "CHOICE_ANSWERED": "choice_answered",
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
        "NAV": "nav",
        "FLUSH_SESSION": "flush_session",
        "PAUSE": "pause",
        "MUTE": "mute",
        "NEXT_SESSION": "next_session",
        "REPEAT": "repeat",
        "JUMP_DECISION": "jump_decision",
        "CATCH_UP": "catch_up",
        "SET_RATE": "set_rate",
        "SET_VERBOSITY": "set_verbosity",
        "SET_VOICE": "set_voice",
        "SET_MINQUEUE": "set_minqueue",
        "SET_AUDIO_CONTROL": "set_audio_control",
        "SET_DUCK_LEVEL": "set_duck_level",
        "SET_VOLUME": "set_volume",
        "SET_SUMMARY_MODE": "set_summary_mode",
        "SET_AUDIO_MODE": "set_audio_mode",
        "SET_SESSION_PREF": "set_session_pref",
        "FORGET_SESSION": "forget_session",
        "SHUTDOWN": "shutdown",
        "STATUS": "status",
        "PING": "ping",
        "REREAD_OPTIONS": "reread_options",
        "CYCLE_VERBOSITY": "cycle_verbosity",
        "RELOAD_KEYMAP": "reload_keymap",
    }
    assert actual == expected


def test_msgtype_values_are_unique():
    values = [
        v for k, v in vars(MsgType).items()
        if not k.startswith("_") and isinstance(v, str)
    ]
    assert len(values) == len(set(values))


def test_reread_options_and_cycle_verbosity_constants():
    assert MsgType.REREAD_OPTIONS == "reread_options"
    assert MsgType.CYCLE_VERBOSITY == "cycle_verbosity"


def test_set_audio_mode_type_exists():
    from sonara.protocol import MsgType
    assert MsgType.SET_AUDIO_MODE == "set_audio_mode"
