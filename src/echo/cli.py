"""Echo command-line interface.

Subcommands fall into two groups:
  * control  -> build a protocol message and hand it to echo.client.send
  * local    -> doctor / install / uninstall / daemon (run in-process)

main(argv) returns an int exit code. Heavy imports (client, daemon) are done
inside the handlers so the module imports cheaply and is easy to patch in tests.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from typing import Optional

from .protocol import MsgType, PROTOCOL_VERSION

VERBOSITY_CHOICES = ("everything", "medium", "quiet")


def _clean_zshrc(path: str) -> bool:
    """Remove legacy claude-tts lines from a zshrc. Returns True if it changed.

    Drops the '# claude-tts' marker comment, the 'alias claude=...claude-speak'
    line, and the '.local/bin' PATH export that carries the '# claude-tts'
    marker. A user's own .local/bin PATH line WITHOUT the marker is preserved.
    """
    p = os.path.expanduser(path)
    if not os.path.exists(p):
        return False
    with open(p, "r", encoding="utf-8") as f:
        lines = f.readlines()

    kept = []
    for line in lines:
        stripped = line.strip()
        if stripped == "# claude-tts":
            continue
        if "claude-speak" in line and "alias" in line and "claude" in line:
            continue
        if ".local/bin" in line and "claude-tts" in line:
            continue
        kept.append(line)

    # Collapse a blank line that the marker block left orphaned at the top of a
    # run only if we actually removed something; otherwise leave file untouched.
    if kept == lines:
        return False

    with open(p, "w", encoding="utf-8") as f:
        f.writelines(kept)
    return True


def _send(msg: dict, expect_reply: bool = False):
    from . import client  # local import so tests can patch echo.client.send
    return client.send(msg, expect_reply=expect_reply)


def _cmd_status(_args) -> int:
    reply = _send({"v": PROTOCOL_VERSION, "type": MsgType.STATUS},
                  expect_reply=True)
    if reply is None:
        print("echo: no response from daemon (is it running?)")
        return 1
    print(json.dumps(reply, indent=2))
    return 0


def _cmd_verbosity(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_VERBOSITY,
           "verbosity": args.level})
    return 0


def _cmd_rate(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_RATE, "rate": args.wpm})
    return 0


def _cmd_voice(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_VOICE, "voice": args.name})
    return 0


def _cmd_repeat(_args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.REPEAT})
    return 0


def _cmd_stop(_args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.STOP})
    return 0


def _cmd_skip(_args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SKIP})
    return 0


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="echo",
                                description="Echo eyes-free TTS for Claude Code")
    sub = p.add_subparsers(dest="command")

    sub.add_parser("status", help="print daemon status").set_defaults(
        func=_cmd_status)

    sp = sub.add_parser("verbosity", help="set verbosity level")
    sp.add_argument("level", choices=VERBOSITY_CHOICES)
    sp.set_defaults(func=_cmd_verbosity)

    sp = sub.add_parser("rate", help="set words-per-minute speech rate")
    sp.add_argument("wpm", type=int)
    sp.set_defaults(func=_cmd_rate)

    sp = sub.add_parser("voice", help="set the say voice")
    sp.add_argument("name")
    sp.set_defaults(func=_cmd_voice)

    sub.add_parser("repeat", help="repeat the last spoken item").set_defaults(
        func=_cmd_repeat)
    sub.add_parser("stop", help="stop all speech and clear the queue").set_defaults(
        func=_cmd_stop)
    sub.add_parser("skip", help="skip the current item").set_defaults(
        func=_cmd_skip)

    # Local subcommands are registered in later tasks via _register_local(sub).
    _register_local(sub)
    return p


def _register_local(sub) -> None:
    """Register local (non-control) subcommands. Filled in later tasks."""
    return None


def main(argv: Optional[list] = None) -> int:
    if argv is None:
        argv = sys.argv[1:]
    parser = _build_parser()
    args = parser.parse_args(argv)
    if not getattr(args, "func", None):
        parser.print_help()
        return 2
    return args.func(args)
