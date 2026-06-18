"""Sonari command-line interface.

Subcommands fall into two groups:
  * control  -> build a protocol message and hand it to sonari.client.send
  * local    -> doctor / install / uninstall / daemon (run in-process)

main(argv) returns an int exit code. Heavy imports (client, daemon) are done
inside the handlers so the module imports cheaply and is easy to patch in tests.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from typing import Optional

from .protocol import MsgType, PROTOCOL_VERSION
from . import paths
from . import keymap
from sonari.platform import get_platform

_PLATFORM = None


def _platform():
    """Return the cached PlatformBackend for this OS (the only OS dispatch point)."""
    global _PLATFORM
    if _PLATFORM is None:
        _PLATFORM = get_platform()
    return _PLATFORM


VERBOSITY_CHOICES = ("everything", "medium", "quiet")


def _send(msg: dict, expect_reply: bool = False):
    from . import client  # local import so tests can patch sonari.client.send
    return client.send(msg, expect_reply=expect_reply)


def _daemon_not_running_message() -> str:
    return "Sonari daemon is not running. Run: sonari install"


def _cmd_status(_args) -> int:
    reply = _send({"v": PROTOCOL_VERSION, "type": MsgType.STATUS},
                  expect_reply=True)
    if reply is None:
        print("sonari: no response from daemon (is it running?)")
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


def _cmd_minqueue(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_MINQUEUE, "minqueue": args.n})
    print("Min queue set to {0}.".format(args.n))
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
            print(f"sonari: could not list voices: {exc}", file=sys.stderr)
            return 1
        if not voices:
            print("No voices installed.")
            return 0
        print("Installed voices (set with: sonari voice <name>):")
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
            print("sonari: usage: sonari keymap [<action> clear]", file=sys.stderr)
            return 2
        try:
            keymap.unbind_action(action)
        except ValueError as exc:
            print(f"sonari: {exc}", file=sys.stderr)
            return 1
        try:                                  # apply live; harmless if daemon is down
            _send({"v": PROTOCOL_VERSION, "type": MsgType.RELOAD_KEYMAP})
        except Exception:  # noqa: BLE001 - the keymap.json write is what matters
            pass
        print(f"Unbound {action}.")
        return 0
    # No args: list EVERY action — bound ones with their combo, the rest "(unbound)".
    try:
        resolved = keymap.resolve_keymap(keymap.load_keymap())
    except ValueError as exc:
        print(f"sonari: invalid keymap: {exc}", file=sys.stderr)
        return 1
    combo_by_action = {
        e["action"]: _combo_label(e["modifiers"], e["keyCode"]) for e in resolved
    }
    for name in keymap.ACTION_MESSAGES:
        print("{0:<16} {1}".format(name, combo_by_action.get(name, "(unbound)")))
    return 0


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="sonari",
                                description="Sonari eyes-free TTS for Claude Code")
    sub = p.add_subparsers(dest="command")

    sub.add_parser("status", help="print daemon status").set_defaults(
        func=_cmd_status)

    sp = sub.add_parser("verbosity", help="set verbosity level")
    sp.add_argument("level", choices=VERBOSITY_CHOICES)
    sp.set_defaults(func=_cmd_verbosity)

    sp = sub.add_parser("rate", help="set words-per-minute speech rate")
    sp.add_argument("wpm", type=int)
    sp.set_defaults(func=_cmd_rate)

    sp = sub.add_parser("voice", help="set the say voice (omit name to list voices)")
    sp.add_argument("name", nargs="*", help="voice name; omit to list installed voices")
    sp.set_defaults(func=_cmd_voice)

    sp = sub.add_parser(
        "minqueue", help="items to batch before reading (1 = read immediately)")
    sp.add_argument("n", type=int)
    sp.set_defaults(func=_cmd_minqueue)

    sub.add_parser("repeat", help="repeat the last spoken item").set_defaults(
        func=_cmd_repeat)
    sub.add_parser("stop", help="stop all speech and clear the queue").set_defaults(
        func=_cmd_stop)
    sub.add_parser("skip", help="skip the current item").set_defaults(
        func=_cmd_skip)

    # Local subcommands are registered in later tasks via _register_local(sub).
    _register_local(sub)
    return p


def doctor() -> list:
    """Return a list of (check, ok, detail) health-check tuples."""
    results = []

    # Platform-specific rows supplied by the OS backend (macOS: say/afplay/
    # swiftc/LaunchAgents/...; Windows: schtasks/Task/pythonw/neural voice/...).
    results.extend(_platform().supervisor.doctor_rows())
    # Hotkey diagnostics (Windows: collisions + UIPI/elevation; macOS: none here).
    results.extend(_platform().hotkey.doctor_rows())

    # Neutral rows (portable, keep inline).
    try:
        paths.ensure_sonari_dir()
        writable = os.access(str(paths.SONARI_DIR), os.W_OK)
        results.append(("SONARI_DIR writable", writable,
                        str(paths.SONARI_DIR) if writable
                        else f"{paths.SONARI_DIR} is not writable"))
    except Exception as exc:  # noqa: BLE001
        results.append(("SONARI_DIR writable", False, f"error: {exc}"))

    try:
        from . import client
        reply = client.send({"v": PROTOCOL_VERSION, "type": MsgType.PING},
                            expect_reply=True)
        ok = bool(reply) and reply.get("ok") is True
        results.append(("daemon socket", ok,
                        "reachable" if ok else "no ok reply from daemon"))
    except Exception as exc:  # noqa: BLE001
        results.append(("daemon socket", False,
                        f"not reachable: {exc} (run 'sonari install')"))

    results.append(_platform().supervisor.hooks_doctor_row())

    try:
        keymap.resolve_keymap(keymap.load_keymap())
        results.append(("keymap resolves", True, "ok"))
    except Exception as exc:  # noqa: BLE001
        results.append(("keymap resolves", False, f"error: {exc}"))

    # python3 >= 3.9 resolved.
    try:
        py = _resolve_python()
        results.append(("python3", py is not None,
                        py or "no python3 >= 3.9 found"))
    except Exception as exc:  # noqa: BLE001
        results.append(("python3", False, f"error: {exc}"))

    # plugin path resolved (install.json -> src contains sonari/__init__.py).
    try:
        rec = _read_install_record()
        app = rec.get("app_path") if rec else None
        init = os.path.join(app, "sonari", "__init__.py") if app else None
        ok = bool(init) and os.path.exists(init)
        results.append(("plugin path resolved", ok,
                        app if ok else "install.json missing or app copy has no "
                                       "sonari package (run 'sonari install')"))
    except Exception as exc:  # noqa: BLE001
        results.append(("plugin path resolved", False, f"error: {exc}"))

    return results


def _cmd_doctor(_args) -> int:
    rows = doctor()
    all_ok = True
    for check, ok, detail in rows:
        mark = "ok " if ok else "FAIL"
        print(f"[{mark}] {check}: {detail}")
        all_ok = all_ok and ok
    return 0 if all_ok else 1


def _resolve_python():
    """Resolve the best Python >= 3.9 via the platform supervisor."""
    return _platform().supervisor.resolve_python()


def _daemon_python(sup):
    """Interpreter the daemon should run on: the neural venv's Python when it is
    provisioned AND probes >=3.10, else the system Python from resolve_python().
    Deriving neural-state from the venv keeps re-runs of `sonari install` on the
    venv interpreter without a separate flag."""
    from sonari import kokoro_provision as kp
    if kp.neural_enabled():
        venv_py = paths.kokoro_venv_python()
        ver = sup._probe_python_version(venv_py)
        if ver is not None and ver >= (3, 10):
            return venv_py
    return sup.resolve_python()


def _write_install_record(python: str, python_version: str,
                          plugin_root: str, app_path: str,
                          plugin_version: str) -> None:
    """Persist the durable install record used by doctor + session-start health."""
    from datetime import datetime, timezone
    record = {
        "python": python,
        "python_version": python_version,
        "app_path": app_path,
        "plugin_root": plugin_root,
        "plugin_version": plugin_version,
        "installed_at": datetime.now(timezone.utc).isoformat(),
    }
    os.makedirs(os.path.dirname(str(paths.INSTALL_RECORD_PATH)), exist_ok=True)
    with open(str(paths.INSTALL_RECORD_PATH), "w", encoding="utf-8") as f:
        json.dump(record, f, indent=2)
        f.write("\n")


def _read_install_record():
    """Return the install.json record dict, or None if unreadable/absent."""
    try:
        with open(str(paths.INSTALL_RECORD_PATH), "r", encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else None
    except Exception:  # noqa: BLE001 - doctor must never raise
        return None


def _read_plugin_version(plugin_root: str) -> str:
    """Return the plugin's declared version, or "" if unreadable.

    Reads <plugin_root>/.claude-plugin/plugin.json 'version'; falls back to the
    CLAUDE_PLUGIN_VERSION env var. Never raises (version is advisory).
    """
    path = os.path.join(plugin_root, ".claude-plugin", "plugin.json")
    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        v = data.get("version") if isinstance(data, dict) else None
        if isinstance(v, str) and v:
            return v
    except Exception:  # noqa: BLE001 - version is advisory, never fatal
        pass
    return os.environ.get("CLAUDE_PLUGIN_VERSION", "") or ""


def _copy_app(plugin_root: str) -> str:
    """Copy the plugin's sonari package into the stable APP_DIR. Returns APP_DIR.

    Overwrites on every install so a plugin update fully refreshes the copy
    (stale modules from a prior version do not linger). The daemon LaunchAgent
    points PYTHONPATH at APP_DIR, decoupling the long-lived daemon from the
    version-pinned marketplace cache.
    """
    app_dir = str(paths.APP_DIR)
    src_pkg = os.path.join(plugin_root, "src", "sonari")
    dst_pkg = os.path.join(app_dir, "sonari")
    os.makedirs(app_dir, exist_ok=True)
    if os.path.isdir(dst_pkg):
        shutil.rmtree(dst_pkg)
    shutil.copytree(src_pkg, dst_pkg)
    return app_dir


def install() -> int:
    """Install Sonari: resolve python, copy the runtime, write the install
    record, then delegate OS-specific autostart + hooks + launcher + hotkeys to
    the platform backend (macOS: LaunchAgents + hotkeyd; Windows: Task Scheduler
    + settings.json hooks + sonari.cmd)."""
    paths.ensure_sonari_dir()
    sup = _platform().supervisor

    # 1. Resolve the best Python >= 3.9 (FATAL if none).
    python = _daemon_python(sup)
    if python is None:
        print("No suitable Python >= 3.9 found. Install Python 3.9+ "
              "(python.org) and re-run: sonari install")
        return 1
    ver = sup._probe_python_version(python)
    py_ver = "{0}.{1}".format(*ver) if ver else "3.9"
    print(f"Using interpreter: {python} (Python {py_ver})")

    plugin_root = os.path.realpath(paths.repo_root())

    # 2. Copy the package into the stable APP_DIR (decouples the long-lived
    #    daemon from the version-pinned marketplace cache; see spec §3.B).
    try:
        app_dir = _copy_app(plugin_root)
    except OSError as exc:
        print(f"Could not copy the runtime to ~/.sonari/app: {exc}. "
              f"Check that ~/.sonari is writable.")
        return 1
    print(f"Copied runtime to: {app_dir}")

    # 3. Keymap setup.
    keymap.write_default_keymap_if_absent()
    keymap.write_resolved()

    # 4. Durable install record.
    plugin_version = _read_plugin_version(plugin_root)
    _write_install_record(python=python, python_version=py_ver,
                          plugin_root=plugin_root, app_path=app_dir,
                          plugin_version=plugin_version)

    # 5. OS-specific autostart + hooks + launcher (the platform backend owns it).
    sup.install(python, app_dir)

    # 6. Global hotkeys. Each backend prints its own outcome (macOS: build +
    #    load hotkeyd; Windows: deferred to M3, announced in post_install_notes).
    hk_log = os.path.join(os.path.dirname(str(paths.LOG_PATH)), "hotkeyd.log")
    launchctl_fn = getattr(sup, "launchctl", None) or (lambda a: 0)
    _platform().hotkey.install(
        log_path=hk_log, agent_path=None, launchctl_fn=launchctl_fn)

    # 7. Voice check (best_voice() is a display-name str on every platform).
    try:
        voice = _platform().tts.best_voice()
        print(f"Voice: {voice}." if voice else "Voice: default.")
    except Exception:  # noqa: BLE001 - voice check must never break install
        pass

    # 8. OS-specific next steps.
    sup.post_install_notes()
    return 0


def _cmd_install(_args) -> int:
    return install()


def uninstall() -> int:
    """Remove Sonari's OS autostart/hooks/launcher (via the platform backend)
    plus the shared runtime artifacts, PRESERVING config.json + keymap.json."""
    sup = _platform().supervisor
    sup.uninstall()
    try:
        _platform().hotkey.uninstall()
    except Exception:  # noqa: BLE001 - hotkey teardown must never break uninstall
        pass

    # Spec §5.4: remove Sonari-owned runtime artifacts but PRESERVE the user's
    # keymap.json AND config.json so customizations survive uninstall/reinstall.
    sonari_dir = paths.SONARI_DIR
    artifacts = [
        paths.LOCK_PATH,
        paths.LOG_PATH,
        paths.HOTKEYD_RESOLVED_PATH,
        paths.INSTALL_RECORD_PATH,
        sonari_dir / "hotkeyd.log",
        sonari_dir / "faulthandler.log",
    ]
    for artifact in artifacts:
        if os.path.exists(str(artifact)):
            try:
                os.remove(str(artifact))
            except OSError:
                pass

    # Remove the stable app copy (spec §3.B). config.json + keymap.json live in
    # SONARI_DIR (not APP_DIR) and are preserved below.
    if os.path.isdir(str(paths.APP_DIR)):
        try:
            shutil.rmtree(str(paths.APP_DIR))
            print(f"Removed app copy: {paths.APP_DIR}")
        except OSError:
            pass

    preserved = []
    if os.path.exists(str(paths.KEYMAP_PATH)):
        preserved.append("keymap.json")
    if os.path.exists(str(paths.CONFIG_PATH)):
        preserved.append("config.json")
    if preserved:
        print(f"Preserved your settings: {', '.join(preserved)}")
    print(f"Removed Sonari runtime files from {sonari_dir} "
          f"(keymap.json and config.json left in place).")

    print("Done. Disable the 'sonari' plugin via /plugin in Claude Code if enabled.")
    return 0


def _cmd_uninstall(_args) -> int:
    return uninstall()


def _cmd_voices_install(_args) -> int:
    """Provision the Kokoro neural-voice venv, then re-wire the daemon onto it."""
    from sonari import kokoro_provision as kp
    paths.ensure_sonari_dir()
    app_dir = str(paths.APP_DIR)
    print("Provisioning neural voices (uv + Kokoro, one-time ~316 MB download)…")
    try:
        kp.install_kokoro(app_dir)
    except Exception as exc:  # noqa: BLE001 - report, do not half-wire
        print(f"Neural-voice setup failed: {exc}", file=sys.stderr)
        return 1
    rc = install()  # re-wires the daemon onto the venv python (neural_enabled() now True)
    if rc == 0 and kp.neural_healthy(app_dir):
        print("Neural voices ready. Pick one with: sonari voice af_heart")
    return rc


def _cmd_voices_uninstall(_args) -> int:
    """Remove the neural venv and revert the daemon to system Python."""
    from sonari import kokoro_provision as kp
    kp.uninstall_kokoro()
    rc = install()  # neural_enabled() now False -> reverts to resolve_python()
    print("Neural voices removed; reverted to the system voice.")
    return rc


def _cmd_daemon(_args) -> int:
    from . import daemon
    daemon.main()
    return 0


def _register_local(sub) -> None:
    """Register local (non-control) subcommands."""
    sub.add_parser("doctor", help="run health checks").set_defaults(
        func=_cmd_doctor)
    sub.add_parser("install", help="install the LaunchAgent + SONARI_DIR").set_defaults(
        func=_cmd_install)
    sub.add_parser("uninstall",
                   help="remove Sonari (LaunchAgents, launcher, runtime files)").set_defaults(
        func=_cmd_uninstall)
    sub.add_parser("daemon", help="run the speech daemon in the foreground").set_defaults(
        func=_cmd_daemon)
    sp = sub.add_parser(
        "keymap",
        help="list hotkey bindings (incl. unbound); '<action> clear' to unbind")
    sp.add_argument("action", nargs="?", help="action to unbind")
    sp.add_argument("value", nargs="?", help="'clear' or 'none' to unbind the action")
    sp.set_defaults(func=_cmd_keymap)
    vp = sub.add_parser("voices", help="install/remove neural (Kokoro) voices")
    vsub = vp.add_subparsers(dest="voices_command")
    vsub.add_parser("install", help="provision neural voices").set_defaults(
        func=_cmd_voices_install)
    vsub.add_parser("uninstall", help="remove neural voices").set_defaults(
        func=_cmd_voices_uninstall)
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
        from .client import DaemonNotRunning  # local import; client may not be loaded
        if isinstance(exc, DaemonNotRunning):
            print(_daemon_not_running_message(), file=sys.stderr)
            return 1
        raise


if __name__ == "__main__":
    sys.exit(main())
