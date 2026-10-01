"""Sonara wire protocol: newline-delimited JSON over a loopback TCP socket
(the port and auth token are in ~/.sonara/daemon.lock). Each message is a dict
with a "type" (one of MsgType) plus that type's fields.

The "v" field is ADVISORY: senders stamp PROTOCOL_VERSION, but the daemon
does not check it, and messages without it (hotkey fires, keymap actions)
are handled the same. A hook running from one plugin copy can talk to a
daemon running from another (~/.sonara/app), and rejecting a version
mismatch would silently drop the latest turn. Changes stay compatible
instead: new types and fields are added, and an unknown type gets no reply."""
from __future__ import annotations

import json

PROTOCOL_VERSION = 1   # stamped as "v"; advisory, never validated (see above)


class MsgType:
    PROSE = "prose"
    CHOICE = "choice"
    CHOICE_ANSWERED = "choice_answered"   # user answered AskUserQuestion (#83)
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
    NAV = "nav"          # Up: msg["to"] == "first" restarts the latest turn (other targets are ignored)
    FLUSH_SESSION = "flush_session"   # hotkey: flush to end, silencing every session (#107)
    PAUSE = "pause"      # toggle play/pause of the whole speak loop
    MUTE = "mute"        # global mute cycle: unmuted -> muted (earcons still fire) -> super muted
    NEXT_SESSION = "next_session"   # hotkey: cycle the active reader to another session
    SET_SESSION_PREF = "set_session_pref"   # {session, key: name|muted|voice, value}
    FORGET_SESSION = "forget_session"       # {session}: drop a stale session everywhere
    REPEAT = "repeat"
    SET_RATE = "set_rate"
    SET_VERBOSITY = "set_verbosity"
    SET_VOICE = "set_voice"
    SET_MINQUEUE = "set_minqueue"
    SET_AUDIO_MODE = "set_audio_mode"         # off | duck | pause (#92)
    SET_DUCK_LEVEL = "set_duck_level"         # set duck target volume (0-100)
    SET_VOLUME = "set_volume"         # speech gain percent (25-200)
    SET_SUMMARY_MODE = "set_summary_mode"     # toggle spoken turn summaries
    SHUTDOWN = "shutdown"   # exit the daemon process cleanly (lifecycle, #23)
    STATUS = "status"
    PING = "ping"
    RELOAD_KEYMAP = "reload_keymap"   # re-read keymap.json + re-register hotkeys


def encode(msg: dict) -> bytes:
    """Serialize a message dict to a newline-terminated UTF-8 byte line."""
    return (json.dumps(msg) + chr(10)).encode("utf-8")


def decode(line: bytes) -> dict:
    """Parse one newline-delimited JSON line back into a dict."""
    return json.loads(line)
