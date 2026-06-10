"""Sonari wire protocol: newline-delimited JSON over a Unix stream socket."""
from __future__ import annotations

import json

PROTOCOL_VERSION = 1


class MsgType:
    PROSE = "prose"
    CHOICE = "choice"
    PLAN = "plan"
    TOOL = "tool_announce"
    PERMISSION = "permission"
    EARCON = "earcon"
    FLUSH = "flush"
    SESSION_START = "session_start"
    SESSION_END = "session_end"
    SET_FOREGROUND = "set_foreground"
    STOP = "stop"
    SKIP = "skip"
    REPEAT = "repeat"
    JUMP_DECISION = "jump_decision"
    CATCH_UP = "catch_up"
    SET_RATE = "set_rate"
    SET_VERBOSITY = "set_verbosity"
    SET_VOICE = "set_voice"
    STATUS = "status"
    PING = "ping"
    REREAD_OPTIONS = "reread_options"
    CYCLE_VERBOSITY = "cycle_verbosity"
    CARET_MOVE = "caret_move"


def encode(msg: dict) -> bytes:
    """Serialize a message dict to a newline-terminated UTF-8 byte line."""
    return (json.dumps(msg) + chr(10)).encode("utf-8")


def decode(line: bytes) -> dict:
    """Parse one newline-delimited JSON line back into a dict."""
    return json.loads(line)
