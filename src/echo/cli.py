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
import shutil
import sys
from typing import Optional

from .protocol import MsgType, PROTOCOL_VERSION
from . import paths

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


def _clean_settings_json(path: str) -> bool:
    """Remove legacy claude-tts hooks from a settings.json. Returns True if changed.

    Drops any hook entry whose command contains 'claude-tts', removes hook
    groups left without hooks, and removes events left without groups. Tolerates
    a missing or corrupt file (returns False, leaves the file untouched).
    """
    p = os.path.expanduser(path)
    if not os.path.exists(p):
        return False
    try:
        with open(p, "r", encoding="utf-8") as f:
            data = json.load(f)
    except (ValueError, OSError):
        return False
    if not isinstance(data, dict):
        return False

    hooks = data.get("hooks")
    if not isinstance(hooks, dict):
        return False

    changed = False
    for event in list(hooks.keys()):
        groups = hooks.get(event)
        if not isinstance(groups, list):
            continue
        new_groups = []
        for group in groups:
            inner = group.get("hooks", []) if isinstance(group, dict) else []
            kept = [h for h in inner
                    if "claude-tts" not in str(h.get("command", ""))]
            if len(kept) != len(inner):
                changed = True
            if not kept:
                # whole group was legacy -> drop it
                continue
            group = dict(group)
            group["hooks"] = kept
            new_groups.append(group)
        if new_groups:
            hooks[event] = new_groups
        else:
            del hooks[event]

    if not changed:
        return False

    with open(p, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
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


def _repo_hooks_json_path() -> str:
    """Path to the plugin hooks/hooks.json inside this repo checkout."""
    here = os.path.dirname(os.path.abspath(__file__))      # src/echo
    repo = os.path.dirname(os.path.dirname(here))          # repo root
    return os.path.join(repo, "hooks", "hooks.json")


def doctor() -> list:
    """Return a list of (check, ok, detail) health-check tuples."""
    results = []

    say = shutil.which("say")
    results.append(("say", say is not None,
                    say or "not found (macOS 'say' required)"))

    afplay = shutil.which("afplay")
    results.append(("afplay", afplay is not None,
                    afplay or "not found (macOS 'afplay' required)"))

    try:
        from . import speaker
        voice = speaker.best_enhanced_voice()
        results.append(("enhanced voice", bool(voice),
                        voice or "none detected; will fall back to Samantha"))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("enhanced voice", False, f"error: {exc}"))

    try:
        paths.ensure_echo_dir()
        writable = os.access(str(paths.ECHO_DIR), os.W_OK)
        results.append(("ECHO_DIR writable", writable,
                        str(paths.ECHO_DIR) if writable
                        else f"{paths.ECHO_DIR} is not writable"))
    except Exception as exc:  # noqa: BLE001
        results.append(("ECHO_DIR writable", False, f"error: {exc}"))

    try:
        from . import client
        reply = client.send({"v": PROTOCOL_VERSION, "type": MsgType.PING},
                            expect_reply=True)
        ok = bool(reply) and reply.get("ok") is True
        results.append(("daemon socket", ok,
                        "reachable" if ok else "no ok reply from daemon"))
    except Exception as exc:  # noqa: BLE001
        results.append(("daemon socket", False,
                        f"not reachable: {exc} (run 'echo install')"))

    hooks_json = _repo_hooks_json_path()
    present = os.path.exists(hooks_json)
    results.append(("plugin hooks.json", present,
                    hooks_json if present else f"missing: {hooks_json}"))

    return results


def _cmd_doctor(_args) -> int:
    rows = doctor()
    all_ok = True
    for check, ok, detail in rows:
        mark = "ok " if ok else "FAIL"
        print(f"[{mark}] {check}: {detail}")
        all_ok = all_ok and ok
    return 0 if all_ok else 1


import subprocess

LAUNCH_AGENT_LABEL = "com.echo.speechd"
LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.echo.speechd.plist")


def _repo_root() -> str:
    here = os.path.dirname(os.path.abspath(__file__))   # src/echo
    return os.path.dirname(os.path.dirname(here))       # repo root


def _daemon_shim_path() -> str:
    return os.path.join(_repo_root(), "bin", "echo-daemon")


def _launchagent_plist(daemon_path: str, log_path: str) -> str:
    """Return the full LaunchAgent plist XML for the speech daemon."""
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
        '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
        '<plist version="1.0">\n'
        '<dict>\n'
        '    <key>Label</key>\n'
        f'    <string>{LAUNCH_AGENT_LABEL}</string>\n'
        '    <key>ProgramArguments</key>\n'
        '    <array>\n'
        f'        <string>{daemon_path}</string>\n'
        '    </array>\n'
        '    <key>RunAtLoad</key>\n'
        '    <true/>\n'
        '    <key>KeepAlive</key>\n'
        '    <true/>\n'
        '    <key>StandardErrorPath</key>\n'
        f'    <string>{log_path}</string>\n'
        '    <key>StandardOutPath</key>\n'
        f'    <string>{log_path}</string>\n'
        '    <key>ProcessType</key>\n'
        '    <string>Interactive</string>\n'
        '</dict>\n'
        '</plist>\n'
    )


def _launchctl(args: list) -> int:
    """Run 'launchctl <args...>'. Patched in tests. Returns the exit code."""
    try:
        return subprocess.call(["launchctl", *args])
    except FileNotFoundError:
        return 1


def install() -> int:
    """Install the speech daemon as a LaunchAgent and ensure ECHO_DIR."""
    paths.ensure_echo_dir()
    daemon = _daemon_shim_path()
    log = str(paths.LOG_PATH)
    xml = _launchagent_plist(daemon, log)

    os.makedirs(os.path.dirname(LAUNCH_AGENT_PATH), exist_ok=True)
    with open(LAUNCH_AGENT_PATH, "w", encoding="utf-8") as f:
        f.write(xml)
    print(f"Wrote LaunchAgent: {LAUNCH_AGENT_PATH}")

    # Reload: unload any prior copy (ignore failure), then load.
    _launchctl(["unload", LAUNCH_AGENT_PATH])
    rc = _launchctl(["load", LAUNCH_AGENT_PATH])
    if rc == 0:
        print(f"Loaded LaunchAgent {LAUNCH_AGENT_LABEL}.")
    else:
        print(f"warning: 'launchctl load' returned {rc}; "
              f"the daemon will still autostart on next login.")

    print("")
    print("Enable the Echo plugin in Claude Code:")
    print(f"  1. Add this repo as a plugin marketplace/source: {_repo_root()}")
    print("  2. In Claude Code run: /plugin")
    print("  3. Enable 'echo' so its hooks load.")
    print("Then run 'echo doctor' to verify everything is wired up.")
    return 0


def _cmd_install(_args) -> int:
    return install()


import shutil as _shutil  # alias so module-level 'shutil' (used by doctor) stays clear


def _legacy_migrate(home: Optional[str] = None) -> list:
    """Clean up a PRIOR legacy claude-tts install. Returns a list of strings
    describing what was removed. Safe (no-op) on a machine with no legacy install.
    """
    base = home or os.path.expanduser("~")
    removed = []

    zshrc = os.path.join(base, ".zshrc")
    if _clean_zshrc(zshrc):
        removed.append(f"cleaned legacy alias/PATH lines from {zshrc}")

    settings = os.path.join(base, ".claude", "settings.json")
    if _clean_settings_json(settings):
        removed.append(f"cleaned legacy hooks from {settings}")

    legacy_files = [
        os.path.join(base, ".local", "bin", "claude-speak"),
        os.path.join(base, ".local", "bin", "claude-tts"),
        os.path.join(base, ".claude-tts-enabled"),
        os.path.join(base, ".claude-tts-pos"),
    ]
    for f in legacy_files:
        if os.path.exists(f):
            try:
                os.remove(f)
                removed.append(f"removed {f}")
            except OSError:
                pass

    return removed


def uninstall() -> int:
    """Remove the LaunchAgent + ECHO_DIR and migrate away a legacy install."""
    if os.path.exists(LAUNCH_AGENT_PATH):
        _launchctl(["unload", LAUNCH_AGENT_PATH])
        try:
            os.remove(LAUNCH_AGENT_PATH)
            print(f"Removed LaunchAgent: {LAUNCH_AGENT_PATH}")
        except OSError as exc:
            print(f"warning: could not remove {LAUNCH_AGENT_PATH}: {exc}")
    else:
        print("No LaunchAgent installed.")

    echo_dir = str(paths.ECHO_DIR)
    if os.path.isdir(echo_dir):
        _shutil.rmtree(echo_dir, ignore_errors=True)
        print(f"Removed {echo_dir}")

    print("Checking for a prior legacy claude-tts install...")
    for line in _legacy_migrate():
        print(f"  - {line}")
    print("Done. Disable the 'echo' plugin via /plugin in Claude Code if enabled.")
    return 0


def _cmd_uninstall(_args) -> int:
    return uninstall()


def _cmd_daemon(_args) -> int:
    from . import daemon
    daemon.main()
    return 0


def _register_local(sub) -> None:
    """Register local (non-control) subcommands."""
    sub.add_parser("doctor", help="run health checks").set_defaults(
        func=_cmd_doctor)
    sub.add_parser("install", help="install the LaunchAgent + ECHO_DIR").set_defaults(
        func=_cmd_install)
    sub.add_parser("uninstall",
                   help="remove Echo and clean a legacy install").set_defaults(
        func=_cmd_uninstall)
    sub.add_parser("daemon", help="run the speech daemon in the foreground").set_defaults(
        func=_cmd_daemon)


def main(argv: Optional[list] = None) -> int:
    if argv is None:
        argv = sys.argv[1:]
    parser = _build_parser()
    args = parser.parse_args(argv)
    if not getattr(args, "func", None):
        parser.print_help()
        return 2
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
