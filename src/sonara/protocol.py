"""Sonara wire protocol: newline-delimited JSON over a Unix stream socket."""
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
    NAV = "nav"          # message-cursor navigation: msg["to"] in next|prev|first|last
    FLUSH_SESSION = "flush_session"   # hotkey: flush the engaged session's queue to the end
    PAUSE = "pause"      # toggle play/pause of the whole speak loop
    MUTE = "mute"        # toggle a sticky per-session mute (earcons still fire)
    NEXT_SESSION = "next_session"   # hotkey: cycle the active reader to another session
    REPEAT = "repeat"
    JUMP_DECISION = "jump_decision"
    CATCH_UP = "catch_up"
    SET_RATE = "set_rate"
    SET_VERBOSITY = "set_verbosity"
    SET_VOICE = "set_voice"
    SET_MINQUEUE = "set_minqueue"
    SET_AUDIO_CONTROL = "set_audio_control"   # enable/disable audio ducking
    SET_DUCK_LEVEL = "set_duck_level"         # set duck target volume (0-100)
    STATUS = "status"
    PING = "ping"
    REREAD_OPTIONS = "reread_options"
    CYCLE_VERBOSITY = "cycle_verbosity"
    RELOAD_KEYMAP = "reload_keymap"   # re-read keymap.json + re-register hotkeys


def encode(msg: dict) -> bytes:
    """Serialize a message dict to a newline-terminated UTF-8 byte line."""
    return (json.dumps(msg) + chr(10)).encode("utf-8")


def decode(line: bytes) -> dict:
    """Parse one newline-delimited JSON line back into a dict."""
    return json.loads(line)
