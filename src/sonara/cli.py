"""Sonara command-line interface.

Subcommands fall into two groups:
  * control  -> build a protocol message and hand it to sonara.client.send
  * local    -> doctor / install / uninstall / daemon (run in-process)

main(argv) returns an int exit code. Heavy imports (client, daemon) are done
inside the handlers so the module imports cheaply and is easy to patch in tests.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from typing import Optional

from .protocol import MsgType, PROTOCOL_VERSION
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


VERBOSITY_CHOICES = ("everything", "medium", "quiet")


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


def _cmd_audio_control(args) -> int:
    enabled = args.state == "on"
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL, "enabled": enabled})
    print("Audio control {0}.".format("on" if enabled else "off"))
    return 0


def _cmd_duck_level(args) -> int:
    _send({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL, "level": args.level})
    print("Duck level set to {0} percent.".format(args.level))
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
    # No args: list EVERY action — bound ones with their combo, the rest "(unbound)".
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

    ap = sub.add_parser("audio-control", help="duck other apps' audio while speaking")
    ap.add_argument("state", choices=["on", "off"])
    ap.set_defaults(func=_cmd_audio_control)

    dp = sub.add_parser("duck-level", help="set duck target volume (0-100)")
    dp.add_argument("level", type=int)
    dp.set_defaults(func=_cmd_duck_level)

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


def doctor() -> list:
    """Return a list of (check, ok, detail) health-check tuples."""
    results = []

    # Platform-specific rows supplied by the OS backend (Windows:
    # schtasks/Task/pythonw/neural voice/...).
    results.extend(_platform().supervisor.doctor_rows())
    # Hotkey diagnostics (Windows: collisions + UIPI/elevation).
    results.extend(_platform().hotkey.doctor_rows())

    # Neutral rows (portable, keep inline).
    try:
        paths.ensure_sonara_dir()
        writable = os.access(str(paths.SONARA_DIR), os.W_OK)
        results.append(("SONARA_DIR writable", writable,
                        str(paths.SONARA_DIR) if writable
                        else f"{paths.SONARA_DIR} is not writable"))
    except Exception as exc:  # noqa: BLE001
        results.append(("SONARA_DIR writable", False, f"error: {exc}"))

    try:
        from . import client
        reply = client.send({"v": PROTOCOL_VERSION, "type": MsgType.PING},
                            expect_reply=True)
        ok = bool(reply) and reply.get("ok") is True
        results.append(("daemon socket", ok,
                        "reachable" if ok else "no ok reply from daemon"))
    except Exception as exc:  # noqa: BLE001
        results.append(("daemon socket", False,
                        f"not reachable: {exc} (run 'sonara start')"))

    results.append(_platform().supervisor.hooks_doctor_row())

    try:
        keymap.resolve_keymap(keymap.load_keymap())
        results.append(("keymap resolves", True, "ok"))
    except Exception as exc:  # noqa: BLE001
        results.append(("keymap resolves", False, f"error: {exc}"))

    # Summary mode: the summarizer command must resolve when the mode is on
    # (the daemon spawns it per turn; a missing command means a failure cue
    # on every turn with no visible cause).
    try:
        from sonara.config import load_config as _load_cfg
        _cfg = _load_cfg()
        if not _cfg.get("summary_mode"):
            results.append(("summary command", True, "summary mode off"))
        else:
            import shutil as _shutil
            _cmd = _cfg.get("summary_command", "claude")
            _found = _shutil.which(_cmd)
            results.append(("summary command", bool(_found),
                            _found or "'{0}' not found on PATH".format(_cmd)))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("summary command", False, f"error: {exc}"))

    try:
        from sonara import kokoro_provision as kp
        if not kp.neural_enabled():
            results.append(("neural voices", True, "not installed (optional)"))
        elif kp.neural_healthy(str(paths.APP_DIR)):
            results.append(("neural voices", True,
                            f"ready ({paths.kokoro_venv_python()})"))
        else:
            results.append(("neural voices", False,
                            "venv present but Kokoro import failed — "
                            "re-run: sonara voices install"))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("neural voices", False, f"error: {exc}"))

    try:
        from sonara import chatterbox as cb
        if not cb.is_provisioned():
            results.append(("chatterbox", True, "not installed (optional)"))
        else:
            py = paths.chatterbox_venv_python()
            voices_dir = str(paths.CHATTERBOX_VOICES_DIR)
            if os.path.exists(py):
                results.append(("chatterbox", True,
                                f"ready ({py}, voices: {voices_dir})"))
            else:
                results.append(("chatterbox", False,
                                f"venv present but missing python at {py} - "
                                "re-run: sonara voices install chatterbox"))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("chatterbox", False, f"error: {exc}"))

    # python3 >= 3.9 resolved.
    try:
        py = _resolve_python()
        results.append(("python3", py is not None,
                        py or "no python3 >= 3.9 found"))
    except Exception as exc:  # noqa: BLE001
        results.append(("python3", False, f"error: {exc}"))

    # plugin path resolved (install.json -> src contains sonara/__init__.py).
    try:
        rec = _read_install_record()
        app = rec.get("app_path") if rec else None
        init = os.path.join(app, "sonara", "__init__.py") if app else None
        ok = bool(init) and os.path.exists(init)
        results.append(("plugin path resolved", ok,
                        app if ok else "install.json missing or app copy has no "
                                       "sonara package (run 'sonara install')"))
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
    Deriving neural-state from the venv keeps re-runs of `sonara install` on the
    venv interpreter without a separate flag."""
    from sonara import kokoro_provision as kp
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
    """Copy the plugin's sonara package into the stable APP_DIR. Returns APP_DIR.

    Overwrites on every install so a plugin update fully refreshes the copy
    (stale modules from a prior version do not linger). The daemon's scheduled
    task points PYTHONPATH at APP_DIR, decoupling the long-lived daemon from the
    version-pinned marketplace cache.
    """
    app_dir = str(paths.APP_DIR)
    src_pkg = os.path.join(plugin_root, "src", "sonara")
    dst_pkg = os.path.join(app_dir, "sonara")
    new_pkg = dst_pkg + ".new"
    old_pkg = dst_pkg + ".old"
    os.makedirs(app_dir, exist_ok=True)
    # Crash-safe swap (#23): build the fresh copy NEXT TO the live one, then
    # rename it in. The old rmtree-then-copytree deleted the live app FIRST, so
    # any failure (classically: the running task's workdir locking a directory)
    # left a gutted install the respawn loop could not run. A failed copytree
    # now leaves the live app untouched.
    for stale in (new_pkg, old_pkg):                # prior-crash residue
        if os.path.isdir(stale):
            shutil.rmtree(stale, ignore_errors=True)
    shutil.copytree(src_pkg, new_pkg)
    if os.path.isdir(dst_pkg):
        os.rename(dst_pkg, old_pkg)
    os.rename(new_pkg, dst_pkg)
    if os.path.isdir(old_pkg):
        shutil.rmtree(old_pkg, ignore_errors=True)  # best-effort; retried next install
    return app_dir


# The Windows speech engine (PyWinRT / OneCore). Kept in sync with the
# [windows] extra in pyproject.toml and the hint in platform/windows/tts.py.
_WINRT_PACKAGES = (
    "winrt-runtime",
    "winrt-Windows.Media.SpeechSynthesis",
    "winrt-Windows.Storage.Streams",
    "pycaw",     # per-app volume control for audio ducking
)


def _winrt_importable(python: str) -> bool:
    """True if PyWinRT's OneCore speech projection imports under *python*."""
    try:
        r = subprocess.run(
            [python, "-c", "import winrt.windows.media.speechsynthesis"],
            capture_output=True, timeout=20)
        return r.returncode == 0
    except Exception:  # noqa: BLE001
        return False


def _ensure_speech_deps(python: str) -> bool:
    """Make sure the Windows speech engine (PyWinRT) is installed in *python*.

    Speech needs the winrt-* packages, and Claude Code does NOT install a plugin's
    optional Python dependencies, so without this step a fresh install is silently
    voiceless. pip-installs them (idempotent: a no-op if already present), then
    verifies. Returns True iff speech can synthesize afterwards."""
    if _winrt_importable(python):
        print("Speech engine (PyWinRT): already installed.")
        return True
    print("Installing the Windows speech engine (PyWinRT)...")
    try:
        subprocess.run([python, "-m", "pip", "install", "--user", *_WINRT_PACKAGES],
                       timeout=300)
    except Exception as exc:  # noqa: BLE001 - fall through to the verify + hint
        print(f"  pip could not run: {exc}")
    if _winrt_importable(python):
        print("Speech engine (PyWinRT): installed.")
        return True
    print("  Could not install PyWinRT automatically. Install it manually:\n    "
          + python + " -m pip install " + " ".join(_WINRT_PACKAGES))
    return False


def stop_sonara(sup=None) -> bool:
    """Stop Sonara everywhere (#23): write the stop sentinel (gates the
    supervisor loop AND the per-hook-event lazy start), end the scheduled task,
    SHUTDOWN the daemon, and wait for it to be gone. Returns True when the
    daemon is confirmed gone (a daemon that was not running counts as stopped).
    install()/uninstall() call this BEFORE mutating files under APP_DIR."""
    paths.ensure_sonara_dir()
    try:
        with open(str(paths.STOPPED_SENTINEL_PATH), "w", encoding="utf-8") as fh:
            fh.write("sonara shutdown")
    except OSError:
        pass
    if sup is None:
        sup = _platform().supervisor
    try:
        sup.end_task()
    except Exception:  # noqa: BLE001 - task may not exist; never fail a stop
        pass
    try:
        _send({"v": PROTOCOL_VERSION, "type": MsgType.SHUTDOWN}, expect_reply=True)
    except Exception:  # noqa: BLE001 - not running IS stopped
        pass
    deadline = time.time() + 5.0
    while time.time() < deadline:
        if not paths.socket_connectable():
            time.sleep(0.3)     # grace: process exit releases the mutex
            return True
        time.sleep(0.1)
    return False


def start_sonara() -> int:
    """Clear a previous shutdown and start the daemon (#23). This is the
    'sonara start' that doctor has always told users to run."""
    try:
        os.remove(str(paths.STOPPED_SENTINEL_PATH))
    except OSError:
        pass
    from sonara import daemon as daemon_module
    daemon_module.ensure_running()
    deadline = time.time() + 5.0
    while time.time() < deadline:
        if paths.socket_connectable():
            print("Sonara daemon is running.")
            return 0
        time.sleep(0.1)
    print("Start requested; the daemon is not accepting yet "
          "(check ~/.sonara/speechd.log).")
    return 1


def _cmd_shutdown(_args) -> int:
    if stop_sonara():
        print("Sonara stopped. It stays stopped until 'sonara start' "
              "(or 'sonara install').")
        return 0
    print("Sonara did not shut down cleanly; a daemon may still be running.")
    return 1


def _cmd_start(_args) -> int:
    return start_sonara()


def install() -> int:
    """Install Sonara: resolve python, ensure the speech engine, copy the runtime,
    write the install record, then delegate OS-specific autostart + hooks +
    launcher + hotkeys to the platform backend (Windows: Task Scheduler +
    settings.json hooks + sonara.cmd)."""
    paths.ensure_sonara_dir()
    sup = _platform().supervisor

    # 1. Resolve the best Python >= 3.9 (FATAL if none).
    python = _daemon_python(sup)
    if python is None:
        print("No suitable Python >= 3.9 found. Install Python 3.9+ "
              "(python.org) and re-run: sonara install")
        return 1
    ver = sup._probe_python_version(python)
    py_ver = "{0}.{1}".format(*ver) if ver else "3.9"
    print(f"Using interpreter: {python} (Python {py_ver})")

    # 1b. Ensure the Windows speech engine (PyWinRT) is installed in that Python.
    #     Claude Code does NOT install a plugin's optional Python deps, so without
    #     this a fresh install is silently voiceless. install() owns it.
    speech_ok = _ensure_speech_deps(python)

    plugin_root = os.path.realpath(paths.repo_root())

    # 1c. STOP Sonara before touching APP_DIR (#23): the scheduled task's
    #     working directory sits INSIDE the tree being replaced, so mutating it
    #     under a running daemon/supervisor half-deleted the app (the documented
    #     'gutted app' failure). The sentinel also blocks a hook event from
    #     lazily respawning the daemon mid-install; cleared after step 5.
    stop_sonara(sup)

    # 2. Copy the package into the stable APP_DIR (decouples the long-lived
    #    daemon from the version-pinned marketplace cache; see spec §3.B).
    try:
        app_dir = _copy_app(plugin_root)
    except OSError as exc:
        print(f"Could not copy the runtime to ~/.sonara/app: {exc}. "
              f"Check that ~/.sonara is writable.")
        return 1
    print(f"Copied runtime to: {app_dir}")

    # 3. Keymap setup.
    keymap.migrate_default_chord()
    keymap.write_default_keymap_if_absent()
    keymap.write_resolved()

    # 4. Durable install record.
    plugin_version = _read_plugin_version(plugin_root)
    _write_install_record(python=python, python_version=py_ver,
                          plugin_root=plugin_root, app_path=app_dir,
                          plugin_version=plugin_version)

    # 5. OS-specific autostart + hooks + launcher (the platform backend owns it).
    sup.install(python, app_dir)

    # 5b. Install complete enough to run: clear the stop sentinel so the next
    #     hook event / logon / 'sonara start' brings the daemon up on the
    #     FRESH code (#23).
    try:
        os.remove(str(paths.STOPPED_SENTINEL_PATH))
    except OSError:
        pass

    # 6. Global hotkeys. Windows hotkeys run in-process and are started by the
    #    daemon (deferred to M3, announced in post_install_notes).
    _platform().hotkey.install()

    # 7. Voice check. Only meaningful once the speech engine is present; otherwise
    #    surface the "add a voice" path so N/KN and bare-Windows users aren't stuck.
    if speech_ok:
        try:
            voice = _platform().tts.best_voice()
            if voice:
                print(f"Voice: {voice}.")
            else:
                print("No speech voice found. Add one in Settings > Time & language "
                      "> Speech > Add voices, then run: sonara doctor")
        except Exception:  # noqa: BLE001 - voice check must never break install
            print("No speech voice found. Add one in Settings > Time & language "
                  "> Speech > Add voices, then run: sonara doctor")

    # 8. OS-specific next steps.
    sup.post_install_notes()

    if not speech_ok:
        print("\n!!  Sonara is set up, but the SPEECH ENGINE is not installed yet, so "
              "it will be SILENT. Install PyWinRT (command above), then re-run "
              "`sonara install` (or `sonara doctor` to confirm).")
        return 1
    return 0


def _cmd_install(_args) -> int:
    return install()


def uninstall() -> int:
    """Remove Sonara's OS autostart/hooks/launcher (via the platform backend)
    plus the shared runtime artifacts, PRESERVING config.json + keymap.json."""
    sup = _platform().supervisor
    # STOP everything FIRST (#23): the old order deleted the task definition and
    # files while the supervisor/daemon kept running (and kept respawning from a
    # deleted install).
    stop_sonara(sup)
    sup.uninstall()
    try:
        _platform().hotkey.uninstall()
    except Exception:  # noqa: BLE001 - hotkey teardown must never break uninstall
        pass

    # Spec §5.4: remove Sonara-owned runtime artifacts but PRESERVE the user's
    # keymap.json AND config.json so customizations survive uninstall/reinstall.
    sonara_dir = paths.SONARA_DIR
    artifacts = [
        paths.LOCK_PATH,
        paths.LOG_PATH,
        paths.HOTKEYD_RESOLVED_PATH,
        paths.INSTALL_RECORD_PATH,
        paths.STOPPED_SENTINEL_PATH,   # clean slate: a reinstall starts fresh (#23)
        sonara_dir / "hotkeyd.log",
        sonara_dir / "faulthandler.log",
    ]
    for artifact in artifacts:
        if os.path.exists(str(artifact)):
            try:
                os.remove(str(artifact))
            except OSError:
                pass

    # Remove the stable app copy (spec §3.B). config.json + keymap.json live in
    # SONARA_DIR (not APP_DIR) and are preserved below.
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
    print(f"Removed Sonara runtime files from {sonara_dir} "
          f"(keymap.json and config.json left in place).")

    print("Done. Disable the 'sonara' plugin via /plugin in Claude Code if enabled.")
    return 0


def _cmd_uninstall(_args) -> int:
    return uninstall()


def _cmd_voices_install(args) -> int:
    """Provision the requested voice engine's venv (kokoro, default; or
    chatterbox, opt-in). Kokoro re-wires the daemon onto its venv; chatterbox
    needs no daemon rewiring (the worker is spawned on demand per-utterance)."""
    engine = getattr(args, "engine", "kokoro") or "kokoro"
    if engine == "chatterbox":
        from sonara import chatterbox_provision as cbp
        paths.ensure_sonara_dir()
        print("Provisioning Chatterbox voices (uv + torch/torchaudio cu128 + "
              "chatterbox-tts, several GB download)…")
        try:
            cbp.install_chatterbox()
        except Exception as exc:  # noqa: BLE001 - report, do not half-install
            print(f"Chatterbox setup failed: {exc}", file=sys.stderr)
            cbp.uninstall_chatterbox()  # revert any half-built venv
            return 1
        except BaseException:
            # Ctrl+C / kill mid-download: still revert -- a half-built venv reads
            # as fully provisioned forever (the python.exe existence check is
            # true after step 1 of a multi-GB install) (audit #21).
            cbp.uninstall_chatterbox()
            raise
        print("Chatterbox voices ready. Pick one with: sonara voice chatterbox:cb_default")
        return 0

    from sonara import kokoro_provision as kp
    paths.ensure_sonara_dir()
    print("Provisioning neural voices (uv + Kokoro, one-time ~316 MB download)…")
    try:
        # Pass repo src as PYTHONPATH so predownload_model can import sonara even
        # before install() populates APP_DIR (on a fresh machine APP_DIR is empty).
        kp.install_kokoro(os.path.join(paths.repo_root(), "src"))
    except Exception as exc:  # noqa: BLE001 - report, do not half-wire
        print(f"Neural-voice setup failed: {exc}", file=sys.stderr)
        kp.uninstall_kokoro()  # revert any half-built venv so neural_enabled() stays False
        return 1
    except BaseException:
        # Ctrl+C / kill mid-download: still revert so neural_enabled() cannot be
        # left True over a half-built venv (audit #21).
        kp.uninstall_kokoro()
        raise
    rc = install()  # re-wires the daemon onto the venv python (neural_enabled() now True)
    if rc == 0 and kp.neural_healthy(str(paths.APP_DIR)):
        print("Neural voices ready. Pick one with: sonara voice af_heart")
    return rc


def _cmd_voices_uninstall(args) -> int:
    """Remove the requested voice engine's venv. Kokoro reverts the daemon to
    system Python; chatterbox needs no daemon rewiring.

    STOP the daemon first (#23): the kokoro venv IS the daemon's interpreter
    (pythonw locks Scripts/) and the chatterbox venv hosts the resident worker;
    deleting either live raised a raw PermissionError and left a half-deleted
    venv that still read as provisioned."""
    engine = getattr(args, "engine", "kokoro") or "kokoro"
    if engine == "chatterbox":
        from sonara import chatterbox_provision as cbp
        stop_sonara()
        cbp.uninstall_chatterbox()
        print("Chatterbox voices removed.")
        start_sonara()   # nothing to rewire; bring the daemon back up
        return 0

    from sonara import kokoro_provision as kp
    stop_sonara()
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
    vp = sub.add_parser("voices", help="install/remove neural (Kokoro/Chatterbox) voices")
    vsub = vp.add_subparsers(dest="voices_command")
    vip = vsub.add_parser("install", help="provision neural voices")
    vip.add_argument("engine", nargs="?", choices=["kokoro", "chatterbox"],
                     default="kokoro", help="voice engine to install (default: kokoro)")
    vip.set_defaults(func=_cmd_voices_install)
    vup = vsub.add_parser("uninstall", help="remove neural voices")
    vup.add_argument("engine", nargs="?", choices=["kokoro", "chatterbox"],
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
        from .client import DaemonNotRunning  # local import; client may not be loaded
        if isinstance(exc, DaemonNotRunning):
            print(_daemon_not_running_message(), file=sys.stderr)
            return 1
        raise


if __name__ == "__main__":
    sys.exit(main())
