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
import re
import shutil
import sys
from typing import Optional

from .protocol import MsgType, PROTOCOL_VERSION
from . import paths
from . import keymap
from sonari.platform.macos.hotkeys import (
    MacHotkeyBackend,
    _KEYCODE_DISPLAY,
    _MOD_DISPLAY,
    LAUNCH_AGENT_LABEL as HOTKEYD_LAUNCH_AGENT_LABEL,
    LAUNCH_AGENT_PATH as HOTKEYD_LAUNCH_AGENT_PATH,
)
from sonari.platform.macos.supervisor import (
    MacSupervisorBackend,
    _PYTHON_CANDIDATE_NAMES,
)
_mac_sup = MacSupervisorBackend()

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


def _combo_label(modifiers: int, key_code: int) -> str:
    parts = [name for mask, name in _MOD_DISPLAY if modifiers & mask]
    parts.append(_KEYCODE_DISPLAY.get(key_code, "key{0}".format(key_code)))
    return "+".join(parts)


def _cmd_keymap(_args) -> int:
    try:
        resolved = keymap.resolve_keymap(keymap.load_keymap())
    except ValueError as exc:
        print(f"sonari: invalid keymap: {exc}", file=sys.stderr)
        return 1
    for entry in resolved:
        combo = _combo_label(entry["modifiers"], entry["keyCode"])
        print("{0:<16} {1}".format(entry["action"], combo))
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
    return os.path.join(paths.repo_root(), "hooks", "hooks.json")


def doctor() -> list:
    """Return a list of (check, ok, detail) health-check tuples."""
    results = []

    # macOS-specific rows: say, afplay, enhanced voice, swiftc, hotkeyd binary,
    # hotkeyd resolved keymap, speechd LaunchAgent loaded, hotkeyd LaunchAgent
    # loaded, sonari launcher.
    results.extend(_mac_sup.doctor_rows())

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

    hooks_json = _repo_hooks_json_path()
    present = os.path.exists(hooks_json)
    results.append(("plugin hooks.json", present,
                    hooks_json if present else f"missing: {hooks_json}"))

    try:
        keymap.resolve_keymap(keymap.load_keymap())
        results.append(("keymap resolves", True, "ok"))
    except Exception as exc:  # noqa: BLE001
        results.append(("keymap resolves", False, f"error: {exc}"))

    # python3 >= 3.9 resolved.
    try:
        py = _resolve_python()
        results.append(("python3", py is not None,
                        py or "no python3 >= 3.9 found; install the Command "
                              "Line Tools (xcode-select --install)"))
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


import subprocess

LAUNCH_AGENT_LABEL = "com.sonari.speechd"
LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.sonari.speechd.plist")

def _repo_root() -> str:
    return paths.repo_root()


def _daemon_shim_path() -> str:
    return os.path.join(paths.repo_root(), "bin", "sonari-daemon")


# _PYTHON_CANDIDATE_NAMES is imported from sonari.platform.macos.supervisor


def _probe_python_version(path: str):
    """Delegating shim — logic lives in MacSupervisorBackend._probe_python_version."""
    return _mac_sup._probe_python_version(path)


def _resolve_python():
    """Delegating shim — logic lives in MacSupervisorBackend.resolve_python."""
    return _mac_sup.resolve_python()


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


def _local_bin_dir() -> str:
    return os.path.join(os.path.expanduser("~"), ".local", "bin")


def _launcher_path() -> str:
    return os.path.join(_local_bin_dir(), "sonari")


def _place_launcher(plugin_root: str) -> str:
    """Delegating shim — logic lives in MacSupervisorBackend.place_launcher."""
    return _mac_sup.place_launcher(plugin_root)


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


def _remove_launcher() -> bool:
    """Remove ~/.local/bin/sonari if present. Returns True if it was removed."""
    path = _launcher_path()
    if os.path.exists(path):
        try:
            os.remove(path)
            return True
        except OSError:
            return False
    return False


def _local_bin_on_path() -> bool:
    """Delegating shim — logic lives in MacSupervisorBackend.local_bin_on_path."""
    return _mac_sup.local_bin_on_path()


def _plist(*a, **k) -> str:
    """Delegating shim — logic lives in MacSupervisorBackend.plist."""
    return _mac_sup.plist(*a, **k)


def _launchagent_plist(python_executable: str, src_path: str,
                       log_path: str) -> str:
    """Delegating shim — logic lives in MacSupervisorBackend.launchagent_plist."""
    return _mac_sup.launchagent_plist(python_executable, src_path, log_path)



def _build_hotkeyd():
    """Compile hotkeyd/sonari-hotkeyd.swift to paths.HOTKEYD_BIN_PATH.

    SKIPS the recompile when the existing binary was already built from the
    current source (same content hash). Recompiling produces a NEW code
    identity, which macOS treats as a different app and re-prompts for any
    permission grants; a routine reinstall (e.g. after a Python-only change)
    must not touch the binary. A real source change re-hashes and rebuilds.
    Returns (ok, detail). Non-fatal: absent swiftc returns
    (False, "swiftc not found").
    """
    return MacHotkeyBackend().build()


def _launchctl(args: list) -> int:
    """Delegating shim — logic lives in MacSupervisorBackend.launchctl."""
    return _mac_sup.launchctl(args)


def install() -> int:
    """Install Sonari as a self-contained plugin: resolve python, build hotkeyd,
    write both LaunchAgents (resolved interp + PYTHONPATH), place the launcher.
    """
    paths.ensure_sonari_dir()

    # 1. Resolve the best python3 >= 3.9 (FATAL if none).
    python = _resolve_python()
    if python is None:
        print("No suitable python3 found (need 3.9+). macOS normally ships "
              "/usr/bin/python3; if missing, install the Command Line Tools "
              "(xcode-select --install).")
        return 1
    ver = _probe_python_version(python)
    py_ver = "{0}.{1}".format(*ver) if ver else "3.9"
    print(f"Using interpreter: {python} (Python {py_ver})")

    plugin_root = os.path.realpath(paths.repo_root())

    # 2. Pre-check swiftc / Command Line Tools (non-fatal).
    if shutil.which("swiftc") is None:
        print("Xcode Command Line Tools not found; global hotkeys disabled. "
              "Install them with:  xcode-select --install   then re-run: "
              "sonari install")

    # 3. Copy the package into the stable APP_DIR (decouples the long-lived
    #    daemon from the version-pinned marketplace cache; see spec §3.B).
    #    Fatal-with-guidance: a half-copy must not produce a dangling plist.
    try:
        app_dir = _copy_app(plugin_root)
    except OSError as exc:
        print(f"Could not copy the runtime to ~/.sonari/app: {exc}. "
              f"Check that ~/.sonari is writable.")
        return 1
    print(f"Copied runtime to: {app_dir}")

    # 4. Keymap setup.
    keymap.write_default_keymap_if_absent()
    keymap.write_resolved()

    # 5. Durable install record.
    plugin_version = _read_plugin_version(plugin_root)
    _write_install_record(python=python, python_version=py_ver,
                          plugin_root=plugin_root, app_path=app_dir,
                          plugin_version=plugin_version)

    # 6. speechd LaunchAgent (resolved interpreter + PYTHONPATH=<APP_DIR>).
    log = str(paths.LOG_PATH)
    xml = _launchagent_plist(python_executable=python, src_path=app_dir,
                             log_path=log)
    os.makedirs(os.path.dirname(LAUNCH_AGENT_PATH), exist_ok=True)
    with open(LAUNCH_AGENT_PATH, "w", encoding="utf-8") as f:
        f.write(xml)
    print(f"Wrote LaunchAgent: {LAUNCH_AGENT_PATH}")
    _launchctl(["unload", LAUNCH_AGENT_PATH])
    rc = _launchctl(["load", LAUNCH_AGENT_PATH])
    if rc == 0:
        print(f"Loaded LaunchAgent {LAUNCH_AGENT_LABEL}.")
    else:
        print(f"warning: 'launchctl load' returned {rc}; "
              f"the daemon will still autostart on next login.")

    # 7. hotkeyd: compile binary + write + load LaunchAgent (skip if no swiftc).
    hk_log = os.path.join(os.path.dirname(str(paths.LOG_PATH)), "hotkeyd.log")
    ok, detail = MacHotkeyBackend().install(
        log_path=hk_log,
        agent_path=HOTKEYD_LAUNCH_AGENT_PATH,
        launchctl_fn=_launchctl,
    )
    if ok:
        print(f"Wrote LaunchAgent: {HOTKEYD_LAUNCH_AGENT_PATH}")
        if detail.startswith("launchctl load returned"):
            print(f"warning: {detail} for the hotkey daemon.")
        else:
            print(f"Loaded LaunchAgent {HOTKEYD_LAUNCH_AGENT_LABEL}.")
    else:
        print(f"warning: hotkey daemon not built ({detail}); "
              f"global hotkeys disabled, but speech still works.")

    # 8. ~/.local/bin/sonari launcher.
    launcher = _place_launcher(plugin_root)
    print(f"Placed launcher: {launcher}")

    # 9. Voice check.
    try:
        from . import speaker
        voice = speaker.best_enhanced_voice()
        if voice:
            print(f"Voice: {voice}.")
        else:
            print("Voice: no enhanced voice found; will fall back to Samantha. "
                  "Install one via System Settings -> Accessibility -> "
                  "Spoken Content.")
    except Exception:  # noqa: BLE001 - voice check must never break install
        pass

    # 10. Eyes-free next steps.
    print("")
    print("Enable the Sonari plugin in Claude Code, then run 'sonari doctor'.")
    print(f"  - Per session: claude --plugin-dir {plugin_root}")
    print("  - Or enable 'sonari' from the /plugin menu (local marketplace).")
    if not _local_bin_on_path():
        print('Add ~/.local/bin to your PATH so `sonari` works in every shell:')
        print('  export PATH="$HOME/.local/bin:$PATH"')
    return 0


def _cmd_install(_args) -> int:
    return install()


def uninstall() -> int:
    """Remove the LaunchAgent + SONARI_DIR (Sonari-owned runtime artifacts)."""
    if os.path.exists(LAUNCH_AGENT_PATH):
        _launchctl(["unload", LAUNCH_AGENT_PATH])
        try:
            os.remove(LAUNCH_AGENT_PATH)
            print(f"Removed LaunchAgent: {LAUNCH_AGENT_PATH}")
        except OSError as exc:
            print(f"warning: could not remove {LAUNCH_AGENT_PATH}: {exc}")
    else:
        print("No LaunchAgent installed.")

    if os.path.exists(HOTKEYD_LAUNCH_AGENT_PATH):
        _launchctl(["unload", HOTKEYD_LAUNCH_AGENT_PATH])
        try:
            os.remove(HOTKEYD_LAUNCH_AGENT_PATH)
            print(f"Removed LaunchAgent: {HOTKEYD_LAUNCH_AGENT_PATH}")
        except OSError as exc:
            print(f"warning: could not remove {HOTKEYD_LAUNCH_AGENT_PATH}: {exc}")
    if os.path.exists(str(paths.HOTKEYD_BIN_PATH)):
        try:
            os.remove(str(paths.HOTKEYD_BIN_PATH))
            print(f"Removed hotkey daemon binary: {paths.HOTKEYD_BIN_PATH}")
        except OSError:
            pass

    # Spec §5.4: remove Sonari-owned runtime artifacts but PRESERVE the user's
    # keymap.json AND config.json so customizations survive uninstall/reinstall.
    sonari_dir = paths.SONARI_DIR
    hk_log = sonari_dir / "hotkeyd.log"
    artifacts = [
        paths.SOCKET_PATH,
        paths.LOG_PATH,
        paths.HOTKEYD_RESOLVED_PATH,
        paths.INSTALL_RECORD_PATH,
        hk_log,
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

    if _remove_launcher():
        print(f"Removed launcher: {_launcher_path()}")

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
    sub.add_parser("keymap",
                   help="print the active global hotkey bindings").set_defaults(
        func=_cmd_keymap)


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
