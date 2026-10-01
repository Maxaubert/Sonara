"""Sonara command-line interface: argparse plus thin command functions.

Subcommands fall into two groups:
  * control  -> build a protocol message and hand it to sonara.client.send
  * local    -> doctor / install / uninstall / voices / cleanup / start /
                shutdown, implemented in sonara.install, and daemon

main(argv) returns an int exit code. Heavy imports (client, daemon, the
install package) are done inside the handlers so the module imports cheaply
and is easy to patch in tests.
"""
from __future__ import annotations

import argparse
import json
import sys
import webbrowser
from typing import Optional

from .protocol import MsgType, PROTOCOL_VERSION
from . import config_schema
from . import paths
from . import keymap
from sonara.platform import get_platform

_PLATFORM = None


def _platform():
    """Return the cached PlatformBackend for this OS (the only OS dispatch point)."""
    global _PLATFORM
    if _PLATFORM is None:
        _PLATFORM = get_platform()
    return _PLATFORM


VERBOSITY_CHOICES = config_schema.VERBOSITY_CHOICES


def _send(msg: dict, expect_reply: bool = False):
    from . import client  # local import so tests can patch sonara.client.send
    return client.send(msg, expect_reply=expect_reply)


def _daemon_not_running_message() -> str:
    return "Sonara daemon is not running. Run: sonara start"


def _cmd_status(_args) -> int:
    reply = _send({"v": PROTOCOL_VERSION, "type": MsgType.STATUS},
                  expect_reply=True)
    if reply is None:
        print("sonara: no response from daemon (is it running?)")
        return 1
    print(json.dumps(reply, indent=2))
    from sonara.platform import transport
    info = transport.read_lockfile(paths.LOCK_PATH)
    if info and info.get("http_port"):
        print("Settings page: http://127.0.0.1:{0}/settings?token={1}".format(
            info["http_port"], info.get("token", "")))
    return 0


def _cmd_settings(_args) -> int:
    """Open the browser settings page at its tokenized URL (#34)."""
    from sonara.platform import transport
    info = transport.read_lockfile(paths.LOCK_PATH)
    if not info or (not info.get("http_port") and not paths.socket_connectable()):
        print(_daemon_not_running_message())
        return 1
    if not info.get("http_port"):
        # E18: the daemon runs, but its settings page failed to start (for
        # example the port was taken). "Not running" sent users the wrong way.
        print("Sonara is running, but its settings page did not start. See "
              "~/.sonara/speechd.log, then restart: sonara shutdown, then "
              "sonara start")
        return 1
    url = "http://127.0.0.1:{0}/settings?token={1}".format(
        info["http_port"], info.get("token", ""))
    print("Settings page: " + url)
    try:
        webbrowser.open(url)
    except Exception:  # noqa: BLE001 - headless/no-browser box: URL is printed anyway
        pass
    return 0


def _cmd_verbosity(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_VERBOSITY,
           "verbosity": args.level})
    return 0


def _cmd_rate(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_RATE, "rate": args.wpm})
    return 0


def _cmd_minqueue(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_MINQUEUE, "minqueue": args.n})
    print("Min queue set to {0}.".format(args.n))
    return 0


def _cmd_duck_level(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL, "level": args.level})
    print("Duck level set to {0} percent.".format(args.level))
    return 0


def _cmd_audio_mode(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_MODE, "mode": args.mode})
    print("Audio mode set to {0}.".format(args.mode))
    return 0


def _cmd_summary(args) -> int:
    if not args.state:
        from sonara.config import load_config
        on = bool(load_config().get("summary_mode"))
        print("Summary mode is {0}.".format("on" if on else "off"))
        return 0
    enabled = args.state == "on"
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_SUMMARY_MODE,
           "enabled": enabled})
    print("Summary mode {0}.".format("on" if enabled else "off"))
    return 0


def _cmd_voice(args) -> int:
    # No name -> list the installed voices so the user can pick one (changes
    # nothing). A name -> set it; the name may be several words ("Microsoft David"),
    # so join them rather than requiring the user to quote.
    name = " ".join(args.name).strip() if args.name else ""
    if not name:
        try:
            voices = _platform().tts.list_voices()
        except Exception as exc:  # noqa: BLE001 - listing must not crash the CLI
            print(f"sonara: could not list voices: {exc}", file=sys.stderr)
            return 1
        if not voices:
            print("No voices installed.")
            return 0
        print("Installed voices (set with: sonara voice <name>):")
        for v in voices:
            print("  " + (getattr(v, "display_name", None) or str(v)))
        return 0
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_VOICE, "voice": name})
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


def _combo_label(modifiers: int, key_code: int) -> str:
    return _platform().hotkey.display_combo(modifiers, key_code)


def _cmd_keymap(args) -> int:
    action = getattr(args, "action", None)
    value = getattr(args, "value", None)
    # `keymap <action> clear|none` -> unbind that action.
    if action:
        if value not in ("clear", "none"):
            print("sonara: usage: sonara keymap [<action> clear]", file=sys.stderr)
            return 2
        try:
            keymap.unbind_action(action)
        except ValueError as exc:
            print(f"sonara: {exc}", file=sys.stderr)
            return 1
        try:                                  # apply live; harmless if daemon is down
            _send({"v": PROTOCOL_VERSION, "type": MsgType.RELOAD_KEYMAP})
        except Exception:  # noqa: BLE001 - the keymap.json write is what matters
            pass
        print(f"Unbound {action}.")
        return 0
    # No args: list EVERY action -- bound ones with their combo, the rest "(unbound)".
    try:
        resolved = keymap.resolve_keymap(keymap.load_keymap())
    except ValueError as exc:
        print(f"sonara: invalid keymap: {exc}", file=sys.stderr)
        return 1
    combo_by_action = {
        e["action"]: _combo_label(e["modifiers"], e["keyCode"]) for e in resolved
    }
    for name in keymap.ACTION_MESSAGES:
        print("{0:<16} {1}".format(name, combo_by_action.get(name, "(unbound)")))
    return 0


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="sonara",
                                description="Sonara eyes-free TTS for Claude Code")
    sub = p.add_subparsers(dest="command")

    sub.add_parser("status", help="print daemon status").set_defaults(
        func=_cmd_status)

    sp = sub.add_parser("verbosity", help="set verbosity level")
    sp.add_argument("level", choices=VERBOSITY_CHOICES)
    sp.set_defaults(func=_cmd_verbosity)

    sp = sub.add_parser("rate", help="set words-per-minute speech rate")
    sp.add_argument("wpm", type=int)
    sp.set_defaults(func=_cmd_rate)

    sp = sub.add_parser("voice", help="set the speech voice (omit name to list voices)")
    sp.add_argument("name", nargs="*", help="voice name; omit to list installed voices")
    sp.set_defaults(func=_cmd_voice)

    sp = sub.add_parser(
        "minqueue", help="items to batch before reading (1 = read immediately)")
    sp.add_argument("n", type=int)
    sp.set_defaults(func=_cmd_minqueue)

    dp = sub.add_parser("duck-level", help="set duck target volume (0-100)")
    dp.add_argument("level", type=int)
    dp.set_defaults(func=_cmd_duck_level)

    am = sub.add_parser("audio-mode", help="off | duck | pause (pause media while speaking)")
    am.add_argument("mode", choices=list(config_schema.AUDIO_MODES))
    am.set_defaults(func=_cmd_audio_mode)

    sp = sub.add_parser(
        "summary", help="speak an AI recap of each finished turn (on|off)")
    sp.add_argument("state", nargs="?", choices=["on", "off"])
    sp.set_defaults(func=_cmd_summary)

    sub.add_parser("repeat", help="repeat the last spoken item").set_defaults(
        func=_cmd_repeat)
    sub.add_parser("stop", help="stop all speech and clear the queue").set_defaults(
        func=_cmd_stop)
    sub.add_parser("skip", help="skip the current item").set_defaults(
        func=_cmd_skip)

    # Local subcommands are registered in later tasks via _register_local(sub).
    _register_local(sub)
    return p


def _cmd_doctor(_args) -> int:
    from sonara.install import doctor
    rows = doctor.doctor()
    all_ok = True
    for check, ok, detail in rows:
        mark = "ok " if ok else "FAIL"
        line = f"[{mark}] {check}: {detail}"
        try:
            print(line)
        except UnicodeEncodeError:
            # A row can name a character (the AltGr row) that a cp437 or
            # redirected console cannot encode: escape it, never crash.
            print(line.encode("ascii", "backslashreplace").decode("ascii"))
        all_ok = all_ok and ok
    return 0 if all_ok else 1


def _cmd_shutdown(_args) -> int:
    from sonara.install import service
    if service.stop_sonara():
        print("Sonara stopped. It stays stopped until 'sonara start' "
              "(or 'sonara install').")
        return 0
    print("Sonara did not shut down cleanly; a daemon may still be running.")
    return 1


def _cmd_start(_args) -> int:
    from sonara.install import service
    return service.start_sonara()


def _cmd_install(_args) -> int:
    from sonara.install import installer
    return installer.install()


def _cmd_uninstall(_args) -> int:
    from sonara.install import installer
    return installer.uninstall()


def _cmd_voices_install(_args) -> int:
    """Provision the Kokoro venv and re-wire the daemon onto it."""
    from sonara.install import voices
    return voices.install_voices()


def _cmd_voices_uninstall(_args) -> int:
    """Remove the Kokoro venv and revert the daemon to system Python."""
    from sonara.install import voices
    return voices.uninstall_voices()


def _cmd_cleanup(_args) -> int:
    """Remove the removed Chatterbox engine's leftovers (#134)."""
    from sonara.install import cleanup
    return cleanup.cleanup()


def _cmd_daemon(_args) -> int:
    from . import daemon
    daemon.main()
    return 0


def _register_local(sub) -> None:
    """Register local (non-control) subcommands."""
    sub.add_parser(
        "settings", help="open the browser settings page").set_defaults(
        func=_cmd_settings)
    sub.add_parser(
        "shutdown",
        help="stop the daemon and supervisor (stays stopped until 'sonara start')",
    ).set_defaults(func=_cmd_shutdown)
    sub.add_parser(
        "start", help="start the daemon (clears a previous shutdown)",
    ).set_defaults(func=_cmd_start)
    sub.add_parser("doctor", help="run health checks").set_defaults(
        func=_cmd_doctor)
    sub.add_parser("install", help="install the scheduled task + SONARA_DIR").set_defaults(
        func=_cmd_install)
    sub.add_parser("uninstall",
                   help="remove Sonara (scheduled task, launcher, runtime files)").set_defaults(
        func=_cmd_uninstall)
    sub.add_parser("daemon", help="run the speech daemon in the foreground").set_defaults(
        func=_cmd_daemon)
    sp = sub.add_parser(
        "keymap",
        help="list hotkey bindings (incl. unbound); '<action> clear' to unbind")
    sp.add_argument("action", nargs="?", help="action to unbind")
    sp.add_argument("value", nargs="?", help="'clear' or 'none' to unbind the action")
    sp.set_defaults(func=_cmd_keymap)
    sub.add_parser(
        "cleanup",
        help="remove leftover Chatterbox files (venv, model cache); keeps voice clips",
    ).set_defaults(func=_cmd_cleanup)
    vp = sub.add_parser("voices", help="install/remove neural (Kokoro) voices")
    vsub = vp.add_subparsers(dest="voices_command")
    vip = vsub.add_parser("install", help="provision neural voices")
    vip.add_argument("engine", nargs="?", choices=["kokoro"],
                     default="kokoro", help="voice engine to install (default: kokoro)")
    vip.set_defaults(func=_cmd_voices_install)
    vup = vsub.add_parser("uninstall", help="remove neural voices")
    vup.add_argument("engine", nargs="?", choices=["kokoro"],
                     default="kokoro", help="voice engine to remove (default: kokoro)")
    vup.set_defaults(func=_cmd_voices_uninstall)
    vp.set_defaults(func=lambda _a: (vp.print_help() or 2))


def main(argv: Optional[list] = None) -> int:
    if argv is None:
        argv = sys.argv[1:]
    parser = _build_parser()
    args = parser.parse_args(argv)
    if not getattr(args, "func", None):
        parser.print_help()
        return 2
    try:
        return args.func(args)
    except OSError as exc:
        from .client import DaemonNotRunning, DaemonUnresponsive  # client may not be loaded
        if isinstance(exc, DaemonNotRunning):
            print(_daemon_not_running_message(), file=sys.stderr)
            return 1
        if isinstance(exc, DaemonUnresponsive):
            # E18: a daemon stuck under its lock used to print a traceback.
            print("Sonara daemon is not responding (busy or stuck). Try again in "
                  "a moment, or restart it: sonara shutdown, then sonara start",
                  file=sys.stderr)
            return 1
        raise


if __name__ == "__main__":
    sys.exit(main())
