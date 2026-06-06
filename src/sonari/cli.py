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


_MOD_DISPLAY = [
    (4096, "Ctrl"),
    (256, "Cmd"),
    (2048, "Opt"),
    (512, "Shift"),
]
_KEYCODE_DISPLAY = {
    1: "S", 15: "R", 2: "D", 37: "L", 9: "V", 31: "O",
    47: ".", 30: "]", 33: "[",
}


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

    swiftc = shutil.which("swiftc")
    results.append(("swiftc", swiftc is not None,
                    swiftc or "not found; install Command Line Tools: "
                              "xcode-select --install"))

    hk_bin = str(paths.HOTKEYD_BIN_PATH)
    hk_exists = os.path.exists(hk_bin)
    results.append(("hotkeyd binary", hk_exists,
                    hk_bin if hk_exists else f"missing: {hk_bin} (run 'sonari install')"))

    try:
        with open(paths.HOTKEYD_RESOLVED_PATH, "r", encoding="utf-8") as fh:
            parsed = json.load(fh)
        ok = isinstance(parsed, list)
        results.append(("hotkeyd resolved keymap", ok,
                        str(paths.HOTKEYD_RESOLVED_PATH) if ok
                        else "not a JSON list"))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("hotkeyd resolved keymap", False, f"unreadable: {exc}"))

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

    # LaunchAgents loaded.
    speechd_loaded = _launchctl(["list", LAUNCH_AGENT_LABEL]) == 0
    results.append(("speechd LaunchAgent loaded", speechd_loaded,
                    LAUNCH_AGENT_LABEL if speechd_loaded
                    else "not loaded (run 'sonari install')"))
    hotkeyd_loaded = _launchctl(["list", HOTKEYD_LAUNCH_AGENT_LABEL]) == 0
    results.append(("hotkeyd LaunchAgent loaded", hotkeyd_loaded,
                    HOTKEYD_LAUNCH_AGENT_LABEL if hotkeyd_loaded
                    else "not loaded (build CLT then 'sonari install')"))

    # ~/.local/bin/sonari launcher + PATH.
    launcher = _launcher_path()
    launcher_ok = os.path.exists(launcher)
    on_path = _local_bin_on_path()
    if launcher_ok and on_path:
        detail = launcher
    elif launcher_ok:
        detail = (f"{launcher} present, but ~/.local/bin is NOT on PATH; add: "
                  'export PATH="$HOME/.local/bin:$PATH"')
    else:
        detail = "missing (run 'sonari install')"
    results.append(("sonari launcher", launcher_ok and on_path, detail))

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

HOTKEYD_LAUNCH_AGENT_LABEL = "com.sonari.hotkeyd"
HOTKEYD_LAUNCH_AGENT_PATH = os.path.expanduser(
    "~/Library/LaunchAgents/com.sonari.hotkeyd.plist")


def _repo_root() -> str:
    return paths.repo_root()


def _daemon_shim_path() -> str:
    return os.path.join(paths.repo_root(), "bin", "sonari-daemon")


_PYTHON_CANDIDATE_NAMES = (
    "python3", "python3.13", "python3.12", "python3.11", "python3.10",
    "python3.9",
)


def _probe_python_version(path: str):
    """Return (major, minor) reported by *path*, or None if it cannot be run.

    Patched in tests. Runs the interpreter so we read its REAL version, not the
    one running cli.py.
    """
    try:
        out = subprocess.check_output(
            [path, "-c", "import sys; print('%d.%d' % sys.version_info[:2])"],
            stderr=subprocess.DEVNULL, text=True, timeout=5).strip()
        major, minor = out.split(".")
        return (int(major), int(minor))
    except Exception:  # noqa: BLE001 - any failure means "not a usable python"
        return None


def _resolve_python():
    """Return the absolute realpath of the best python3 >= 3.9, or None.

    Preference: /usr/bin/python3 when it qualifies (guaranteed present and stable
    across logins); otherwise the first qualifying candidate in PATH order.
    Candidates are deduped by realpath so a symlink farm is probed once.
    """
    candidates = ["/usr/bin/python3"]
    for name in _PYTHON_CANDIDATE_NAMES:
        found = shutil.which(name)
        if found:
            candidates.append(found)

    seen = set()
    qualifying = []  # list of (realpath, was_usr_bin)
    for cand in candidates:
        real = os.path.realpath(cand)
        if real in seen:
            continue
        seen.add(real)
        ver = _probe_python_version(cand)
        if ver is not None and ver >= (3, 9):
            qualifying.append((real, cand == "/usr/bin/python3"))

    if not qualifying:
        return None
    for real, was_usr_bin in qualifying:
        if was_usr_bin:
            return real
    return qualifying[0][0]


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
    """Write an executable ~/.local/bin/sonari that execs the plugin bin/sonari.

    The plugin path is baked in and shell-quoted so a path with spaces is safe.
    Overwrites any prior Sonari-owned launcher. Returns the launcher path.
    """
    target = os.path.join(plugin_root, "bin", "sonari")
    wrapper = (
        "#!/usr/bin/env bash\n"
        f'exec "{target}" "$@"\n'
    )
    path = _launcher_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(wrapper)
    os.chmod(path, 0o755)
    return path


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
    """Return True if ~/.local/bin is on the current PATH."""
    lb = _local_bin_dir()
    entries = os.environ.get("PATH", "").split(os.pathsep)
    return lb in entries


def _xml_escape(s: str) -> str:
    """Escape the three XML-significant characters for safe plist interpolation."""
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _plist(label: str, program_args: list, log_path: str,
           env: Optional[dict] = None) -> str:
    """Return a full LaunchAgent plist XML for *label*.

    *program_args* is the ProgramArguments array (already absolute paths).
    *env*, when given, is emitted as an EnvironmentVariables <dict> (used to
    inject PYTHONPATH for the self-contained speech daemon). Every interpolated
    string is XML-escaped so a path containing &, <, or > cannot corrupt the
    plist. RunAtLoad + KeepAlive keep the agent alive in the Aqua (GUI) session;
    ProcessType Interactive so it participates in the foreground session that
    Carbon hotkeys require.
    """
    args_xml = "".join(
        f"        <string>{_xml_escape(a)}</string>\n" for a in program_args)
    env_xml = ""
    if env:
        pairs = "".join(
            f"        <key>{_xml_escape(k)}</key>\n"
            f"        <string>{_xml_escape(v)}</string>\n"
            for k, v in env.items())
        env_xml = (
            '    <key>EnvironmentVariables</key>\n'
            '    <dict>\n'
            f'{pairs}'
            '    </dict>\n'
        )
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
        '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
        '<plist version="1.0">\n'
        '<dict>\n'
        '    <key>Label</key>\n'
        f'    <string>{_xml_escape(label)}</string>\n'
        '    <key>ProgramArguments</key>\n'
        '    <array>\n'
        f'{args_xml}'
        '    </array>\n'
        f'{env_xml}'
        '    <key>RunAtLoad</key>\n'
        '    <true/>\n'
        '    <key>KeepAlive</key>\n'
        '    <true/>\n'
        '    <key>StandardErrorPath</key>\n'
        f'    <string>{_xml_escape(log_path)}</string>\n'
        '    <key>StandardOutPath</key>\n'
        f'    <string>{_xml_escape(log_path)}</string>\n'
        '    <key>ProcessType</key>\n'
        '    <string>Interactive</string>\n'
        '</dict>\n'
        '</plist>\n'
    )


def _launchagent_plist(python_executable: str, src_path: str,
                       log_path: str) -> str:
    """Return the LaunchAgent plist XML for the speech daemon.

    *python_executable* is the resolved absolute interpreter (>= 3.9).
    *src_path* is the stable APP_DIR copy (~/.sonari/app); it is injected as
    PYTHONPATH so the daemon imports the stable package copy, surviving plugin
    cache churn. ProgramArguments runs the module directly: [<py>, -m, sonari.daemon].
    """
    return _plist(
        LAUNCH_AGENT_LABEL,
        [python_executable, "-m", "sonari.daemon"],
        log_path,
        env={"PYTHONPATH": src_path},
    )


def _hotkeyd_plist(binary_path: str, log_path: str) -> str:
    """Return the full LaunchAgent plist XML for the hotkey daemon.

    Runs the compiled Swift binary directly.
    """
    return _plist(HOTKEYD_LAUNCH_AGENT_LABEL, [binary_path], log_path)


def _build_hotkeyd():
    """Compile hotkeyd/sonari-hotkeyd.swift to paths.HOTKEYD_BIN_PATH.

    Returns (ok, detail). Non-fatal: if swiftc is absent we return
    (False, "swiftc not found") and the caller warns but still installs speechd.
    """
    if shutil.which("swiftc") is None:
        return (False, "swiftc not found")
    src = os.path.join(paths.repo_root(), "hotkeyd", "sonari-hotkeyd.swift")
    rc = subprocess.call(["swiftc", src, "-o", str(paths.HOTKEYD_BIN_PATH)])
    if rc == 0:
        return (True, str(paths.HOTKEYD_BIN_PATH))
    return (False, f"swiftc exited {rc}")


def _launchctl(args: list) -> int:
    """Run 'launchctl <args...>'. Patched in tests. Returns the exit code."""
    try:
        return subprocess.call(["launchctl", *args])
    except FileNotFoundError:
        return 1


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

    # 4. Keymap + build hotkeyd.
    keymap.write_default_keymap_if_absent()
    keymap.write_resolved()
    ok, detail = _build_hotkeyd()

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

    # 7. hotkeyd LaunchAgent (skip entirely if no binary).
    if ok:
        hk_log = os.path.join(os.path.dirname(str(paths.LOG_PATH)), "hotkeyd.log")
        hk_xml = _hotkeyd_plist(str(paths.HOTKEYD_BIN_PATH), hk_log)
        os.makedirs(os.path.dirname(HOTKEYD_LAUNCH_AGENT_PATH), exist_ok=True)
        with open(HOTKEYD_LAUNCH_AGENT_PATH, "w", encoding="utf-8") as f:
            f.write(hk_xml)
        print(f"Wrote LaunchAgent: {HOTKEYD_LAUNCH_AGENT_PATH}")
        _launchctl(["unload", HOTKEYD_LAUNCH_AGENT_PATH])
        hrc = _launchctl(["load", HOTKEYD_LAUNCH_AGENT_PATH])
        if hrc == 0:
            print(f"Loaded LaunchAgent {HOTKEYD_LAUNCH_AGENT_LABEL}.")
        else:
            print(f"warning: 'launchctl load' returned {hrc} for the hotkey daemon.")
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
