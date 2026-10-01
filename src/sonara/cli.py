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
import webbrowser
from typing import Optional

from .protocol import MsgType, PROTOCOL_VERSION
from . import config_schema
from . import paths
from . import install_record
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
            _cmd = config_schema.get(_cfg, "summary_command")
            _found = _shutil.which(_cmd)
            results.append(("summary command", bool(_found),
                            _found or "'{0}' not found on PATH".format(_cmd)))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("summary command", False, f"error: {exc}"))

    try:
        results.append(_neural_voices_row())
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("neural voices", False, f"error: {exc}"))

    # Leftovers of the removed Chatterbox engine (#134): several GB that only
    # `sonara cleanup` deletes. Informational, so never a failing row.
    try:
        # Bounded walk: the old venv alone is ~8 GB of small files, too slow
        # to count on every doctor run, so past a cap it says "more than".
        from sonara import chatterbox_legacy as cl
        found = cl.leftovers_estimate()
        if not found:
            results.append(("chatterbox leftovers", True, "none"))
        else:
            def _fmt(size, exact):
                if exact:
                    return cl.format_size(size)
                return "more than " + cl.format_size(size) if size else "large"
            total = sum(size for _p, size, _e in found)
            parts = ", ".join("{0} ({1})".format(p.name, _fmt(size, exact))
                              for p, size, exact in found)
            results.append(("chatterbox leftovers", True,
                            "{0} reclaimable: {1}. Remove with: sonara cleanup "
                            "(your voice clips are kept)".format(
                                _fmt(total, all(e for _p, _s, e in found)),
                                parts)))
    except Exception as exc:  # noqa: BLE001 - doctor must never raise
        results.append(("chatterbox leftovers", True, f"could not check: {exc}"))

    # python3 >= 3.9 resolved.
    try:
        py = _resolve_python()
        results.append(("python3", py is not None,
                        py or "no python3 >= 3.9 found"))
    except Exception as exc:  # noqa: BLE001
        results.append(("python3", False, f"error: {exc}"))

    # plugin path resolved (install.json -> src contains sonara/__init__.py).
    try:
        rec = install_record.read()
        app = rec.get("app_path") if rec else None
        init = os.path.join(app, "sonara", "__init__.py") if app else None
        ok = bool(init) and os.path.exists(init)
        results.append(("plugin path resolved", ok,
                        app if ok else "install.json missing or app copy has no "
                                       "sonara package (run 'sonara install')"))
    except Exception as exc:  # noqa: BLE001
        results.append(("plugin path resolved", False, f"error: {exc}"))

    return results


def _neural_voices_row() -> tuple:
    """Kokoro as the DAEMON sees it: the neural venv when provisioned, else
    the daemon's own interpreter (install.json), which may have Kokoro in its
    site-packages. Plus where the model stands (downloaded, pending, or a
    recent failed download)."""
    from sonara import kokoro_provision as kp
    if kp.neural_enabled():
        if not kp.neural_healthy(str(paths.APP_DIR)):
            return ("neural voices", False,
                    "venv present but Kokoro import failed - "
                    "re-run: sonara voices install")
        where = paths.kokoro_venv_python()
    else:
        rec = install_record.read() or {}
        where = rec.get("python")
        if not where or not kp.kokoro_importable(where):
            return ("neural voices", True,
                    "not installed (optional): sonara voices install")
    from sonara import kokoro
    model_dir = paths.SONARA_DIR / "kokoro"
    if kokoro.models_present(model_dir):
        return ("neural voices", True, "ready ({0})".format(where))
    failed = kokoro.download_failed_at(model_dir)
    if failed is not None and time.time() - failed < kokoro.DOWNLOAD_RETRY_S:
        return ("neural voices", False,
                "Kokoro is installed ({0}) but the model download failed at {1}; "
                "Windows voices stand in. Retry now: sonara voices install".format(
                    where, time.strftime("%H:%M", time.localtime(failed))))
    return ("neural voices", True,
            "ready ({0}); the ~316 MB model downloads on first use".format(where))


def _cmd_doctor(_args) -> int:
    rows = doctor()
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


def _resolve_python():
    """Resolve the best Python >= 3.9 via the platform supervisor."""
    return _platform().supervisor.resolve_python()


def _daemon_python(sup):
    """Interpreter the daemon should run on: the neural venv's Python when it is
    provisioned AND probes >=3.10, else the system Python from resolve_python().
    Deriving neural-state from the venv keeps re-runs of `sonara install` on the
    venv interpreter without a separate flag."""
    from sonara import kokoro_provision as kp
    return kp.usable_venv_python(sup._probe_python_version) or sup.resolve_python()


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
    # Repo bytecode is dead weight in the app and can mask a source change.
    shutil.copytree(src_pkg, new_pkg,
                    ignore=shutil.ignore_patterns("__pycache__"))
    if os.path.isdir(dst_pkg):
        _rename_retrying(dst_pkg, old_pkg)
    try:
        _rename_retrying(new_pkg, dst_pkg)
    except OSError:
        # #127: the old package is already aside. Put it back so a live
        # 'sonara' package always exists; the fresh copy stays as residue
        # that the next install sweeps.
        if os.path.isdir(old_pkg) and not os.path.isdir(dst_pkg):
            _rename_retrying(old_pkg, dst_pkg)
        raise
    if os.path.isdir(old_pkg):
        shutil.rmtree(old_pkg, ignore_errors=True)  # best-effort; retried next install
    return app_dir


def _rename_retrying(src: str, dst: str, attempts: int = 10,
                     delay: float = 0.3) -> None:
    """os.rename that retries a PermissionError. On Windows a just-written tree
    (antivirus, indexer) or one a just-exited daemon still pins is often denied
    for a moment, and a retry a little later succeeds (#127)."""
    for attempt in range(attempts):
        try:
            os.rename(src, dst)
            return
        except PermissionError:
            if attempt == attempts - 1:
                raise
            time.sleep(delay)


def _is_plugin_root(path) -> bool:
    """True if *path* is a plugin checkout install() can deploy from: it has the
    package source, the hook entry the hooks point at, and the hooks file."""
    if not path:
        return False
    return (os.path.isfile(os.path.join(path, "src", "sonara", "__init__.py"))
            and os.path.isfile(os.path.join(path, "bin", "sonara-hook"))
            and os.path.isfile(os.path.join(path, "hooks", "hooks.json")))


def _print_no_plugin_root() -> None:
    print("Cannot find the Sonara plugin files (src/sonara, bin/sonara-hook, "
          "hooks/hooks.json) next to this copy of Sonara, which looks like "
          "the deployed runtime in ~/.sonara. Nothing was changed. Run "
          "/sonara:install in Claude Code, or <plugin folder>/bin/sonara install.")


def _resolve_plugin_root() -> Optional[str]:
    """The plugin tree install() deploys from, or None.

    repo_root() is right when the CLI runs from a checkout or the plugin
    cache. From the deployed copy (the ~/.local/bin launcher) it resolves to
    ~/.sonara, which has no src/ and no bin/ (H3), so fall back to the plugin
    Claude Code names (CLAUDE_PLUGIN_ROOT), then to the one the last install
    recorded."""
    record = install_record.read() or {}
    for cand in (paths.repo_root(), os.environ.get("CLAUDE_PLUGIN_ROOT"),
                 record.get("plugin_root")):
        if isinstance(cand, str) and _is_plugin_root(cand):
            return os.path.realpath(cand)
    return None


# The Windows speech engine (PyWinRT / OneCore). Kept in sync with the
# [windows] extra in pyproject.toml, requirements-kokoro.txt (the neural venv)
# and the hint in platform/windows/tts.py.
_WINRT_PACKAGES = (
    "winrt-runtime",
    "winrt-Windows.Media.SpeechSynthesis",
    "winrt-Windows.Storage.Streams",
    "winrt-Windows.Media.Control",   # SMTC pause/resume of other apps' media (#92)
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
    console = _console_sibling(python)
    cmd = _speech_install_cmd(console, _python_env(console), _find_uv())
    try:
        subprocess.run(cmd, timeout=300)
    except Exception as exc:  # noqa: BLE001 - fall through to the verify + hint
        print(f"  the installer could not run: {exc}")
    if _winrt_importable(python):
        print("Speech engine (PyWinRT): installed.")
        return True
    print("  Could not install PyWinRT automatically. Install it manually:\n    "
          + " ".join(cmd))
    return False


def _console_sibling(python: str) -> str:
    """python.exe next to a pythonw.exe (installers and uv want the console
    interpreter); *python* itself otherwise."""
    head, tail = os.path.split(python)
    if tail.lower() == "pythonw.exe":
        cand = os.path.join(head, "python.exe")
        if os.path.isfile(cand):
            return cand
    return python


_PY_ENV_PROBE = (
    "import importlib.util, json, os, sys, sysconfig; print(json.dumps({"
    "'venv': sys.prefix != sys.base_prefix, "
    "'managed': os.path.isfile(os.path.join("
    "sysconfig.get_path('stdlib'), 'EXTERNALLY-MANAGED')), "
    "'pip': importlib.util.find_spec('pip') is not None}))")


def _python_env(python: str) -> dict:
    """What kind of interpreter *python* is: {'venv', 'managed', 'pip'} as
    booleans ({} when the probe fails, which reads as a plain system Python).
    'managed' is a PEP 668 EXTERNALLY-MANAGED marker, as uv's own Pythons carry."""
    try:
        r = subprocess.run([python, "-c", _PY_ENV_PROBE], capture_output=True,
                           text=True, timeout=20)
        data = json.loads(r.stdout)
        return data if isinstance(data, dict) else {}
    except Exception:  # noqa: BLE001 - an unknown interpreter keeps the old path
        return {}


def _find_uv() -> Optional[str]:
    """uv on PATH, else the copy the bootstrap downloaded to ~/.sonara/tools."""
    found = shutil.which("uv")
    if found:
        return found
    local = os.path.join(str(paths.SONARA_DIR), "tools", "uv.exe")
    return local if os.path.isfile(local) else None


def _speech_install_cmd(python: str, env: dict, uv: Optional[str]) -> list:
    """The command that installs _WINRT_PACKAGES into *python*.

    `pip install --user` only fits a plain system Python. A uv-managed Python
    is externally managed (PEP 668) and refuses it (E1); a venv rejects
    --user, and a uv venv has no pip at all (E2). uv installs into either."""
    pkgs = list(_WINRT_PACKAGES)
    if env.get("venv"):
        if env.get("pip") or not uv:
            return [python, "-m", "pip", "install", *pkgs]
        return [uv, "pip", "install", "--python", python, *pkgs]
    if env.get("managed"):
        if uv:
            return [uv, "pip", "install", "--python", python,
                    "--break-system-packages", *pkgs]
        return [python, "-m", "pip", "install", "--user",
                "--break-system-packages", *pkgs]
    return [python, "-m", "pip", "install", "--user", *pkgs]


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
            _kill_stray_daemons()
            return True
        time.sleep(0.1)
    return False


def _kill_stray_daemons(runner=None) -> int:
    """Terminate any `-m sonara.daemon` processes still alive after the socket
    owner died (#65). SHUTDOWN only reaches the lockfile/socket owner; a
    split-brain survivor (an older daemon that lost the socket race but still
    holds the global hotkeys) outlives every `sonara shutdown` and keeps
    swallowing hotkey presses - mute appears broken. Best-effort: returns the
    number of processes killed, 0 on any failure or on non-Windows."""
    if os.name != "nt":
        return 0
    script = (
        "Get-CimInstance Win32_Process -Filter \"Name like 'python%'\" | "
        "Where-Object { $_.CommandLine -match 'sonara[.]daemon' } | "
        "ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue; "
        "$_.ProcessId }")
    run = runner or (lambda argv: subprocess.run(
        argv, capture_output=True, timeout=15,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)))
    try:
        proc = run(["powershell", "-NoProfile", "-Command", script])
        killed = [ln for ln in (proc.stdout or b"").decode("utf-8", "replace").split()
                  if ln.strip().isdigit()]
        if killed:
            print("stopped {0} stray daemon process(es): {1}".format(
                len(killed), ", ".join(killed)))
        return len(killed)
    except Exception:  # noqa: BLE001 - a failed sweep must never fail a stop
        return 0


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


def _install_runtime(sup, python: str, py_ver: str, plugin_root: str):
    """install() steps 2-5, run while Sonara is stopped: copy the runtime,
    keymap, install record, then the OS autostart + hooks + launcher. Returns
    APP_DIR, or None after printing why it failed. Never raises an ordinary
    error: install() must get the chance to clear the stop sentinel."""
    # 2. Copy the package into the stable APP_DIR (decouples the long-lived
    #    daemon from the version-pinned marketplace cache; see spec §3.B).
    try:
        app_dir = _copy_app(plugin_root)
    except OSError as exc:
        print(f"Could not copy the runtime to ~/.sonara/app: {exc}. "
              f"Check that ~/.sonara is writable, then re-run: sonara install")
        return None
    print(f"Copied runtime to: {app_dir}")
    try:
        # 3. Keymap setup.
        keymap.migrate_default_chord()
        keymap.write_default_keymap_if_absent()

        # 4. Durable install record.
        plugin_version = _read_plugin_version(plugin_root)
        install_record.write(python=python, python_version=py_ver,
                             plugin_root=plugin_root, app_path=app_dir,
                             plugin_version=plugin_version)

        # 5. OS-specific autostart + hooks + launcher (the platform backend
        #    owns it). ValueError here is an unparseable ~/.claude/settings.json.
        sup.install(python, app_dir, plugin_root=plugin_root)
    except Exception as exc:  # noqa: BLE001 - report, never a traceback mid-install
        print(f"Install did not finish: {exc}\nFix that, then re-run: sonara install")
        return None
    return app_dir


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

    # 1a. Find the plugin tree BEFORE changing anything: from the deployed
    #     copy there may be none, and stopping first left Sonara off (H3).
    plugin_root = _resolve_plugin_root()
    if plugin_root is None:
        _print_no_plugin_root()
        return 1

    # 1b. Ensure the Windows speech engine (PyWinRT) is installed in that Python.
    #     Claude Code does NOT install a plugin's optional Python deps, so without
    #     this a fresh install is silently voiceless. install() owns it.
    speech_ok = _ensure_speech_deps(python)

    # 1c. STOP Sonara before touching APP_DIR (#23): the scheduled task's
    #     working directory sits INSIDE the tree being replaced, so mutating it
    #     under a running daemon/supervisor half-deleted the app (the documented
    #     'gutted app' failure). The sentinel also blocks a hook event from
    #     lazily respawning the daemon mid-install. It is cleared whether the
    #     steps below succeed or fail (E6): a failed install that left it in
    #     place kept Sonara off with no cue.
    stop_sonara(sup)
    try:
        app_dir = _install_runtime(sup, python, py_ver, plugin_root)
    finally:
        try:
            os.remove(str(paths.STOPPED_SENTINEL_PATH))
        except OSError:
            pass
    if app_dir is None:
        return 1

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
        paths.INSTALL_RECORD_PATH,
        sonara_dir / "speechd.old.log",   # the rotated log (L-log)
        # Legacy files from the removed macOS hotkeyd; older installs still
        # wrote hotkeyd.resolved.json, so uninstall keeps sweeping them.
        sonara_dir / "hotkeyd.resolved.json",
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
    print(f"Removed Sonara's runtime files from {sonara_dir} "
          f"(keymap.json and config.json left in place).")
    _print_uninstall_leftovers()

    # E8: keep Sonara STOPPED. With the plugin still enabled, the very next
    # hook event would otherwise lazily start a daemon from the plugin's code
    # right after this uninstall. install() and 'sonara start' clear it.
    try:
        paths.ensure_sonara_dir()
        with open(str(paths.STOPPED_SENTINEL_PATH), "w", encoding="utf-8") as fh:
            fh.write("sonara uninstall")
    except OSError:
        pass

    print("Done. Sonara stays off. Now disable the 'sonara' plugin via /plugin in "
          "Claude Code (if it is enabled), or its hooks keep running.")
    return 0


def _print_uninstall_leftovers() -> None:
    """Say what uninstall deliberately keeps in ~/.sonara and how to remove it
    (E19). Never raises."""
    try:
        neural = [os.path.join(str(paths.SONARA_DIR), d) for d in ("venv", "kokoro")]
        neural = [d for d in neural if os.path.isdir(d)]
        if neural:
            print("Neural voices are kept in {0}; delete those folders to free "
                  "the space.".format(" and ".join(neural)))
        from sonara import chatterbox_legacy as cl
        found = cl.leftovers()
        if found:
            print("Old Chatterbox files remain ({0}); 'sonara cleanup' removes "
                  "them.".format(cl.format_size(sum(s for _p, s in found))))
        print("Anything else left in {0} (logs, caches, voice clips) can be "
              "deleted by hand once Sonara is off.".format(paths.SONARA_DIR))
    except Exception:  # noqa: BLE001 - an advisory note must never fail uninstall
        pass


def _cmd_uninstall(_args) -> int:
    return uninstall()


def _stopped_state_restorer():
    """Snapshot whether Sonara runs now, for a command that stops it and may
    then fail: the returned callable starts it again if it was running, or
    removes the stop sentinel the command's own stop_sonara wrote, so a
    failure never leaves Sonara off. An earlier explicit shutdown stays."""
    was_running = paths.socket_connectable()
    was_shut_down = os.path.exists(str(paths.STOPPED_SENTINEL_PATH))

    def restore():
        if was_running:
            start_sonara()
        elif not was_shut_down:
            try:
                os.remove(str(paths.STOPPED_SENTINEL_PATH))
            except OSError:
                pass
    return restore


def _cmd_voices_install(_args) -> int:
    """Provision the Kokoro venv and re-wire the daemon onto it."""
    from sonara import kokoro_provision as kp
    # install() below refuses without a plugin tree (H3): check before the
    # ~316 MB download, not after it.
    if _resolve_plugin_root() is None:
        _print_no_plugin_root()
        return 1
    paths.ensure_sonara_dir()
    # E10: the daemon may be running on this very venv's pythonw, whose files
    # are then locked. Stop it first, and only ever remove a venv this run
    # created: a failed re-run or upgrade keeps the one that worked.
    existed = kp.neural_enabled()
    restore = _stopped_state_restorer()
    stop_sonara()
    print("Provisioning neural voices (uv + Kokoro, one-time ~316 MB download)…")
    try:
        # Pass the running package's root as PYTHONPATH so predownload_model can
        # import sonara even before install() populates APP_DIR (on a fresh
        # machine APP_DIR is empty). repo_root()/src was ~/.sonara/src, which
        # does not exist, when this ran from the deployed copy (E6).
        kp.install_kokoro(paths.package_root())
    except BaseException as exc:
        # Ctrl+C / kill mid-download too: a venv this run created is reverted
        # so neural_enabled() cannot be left True over a half-built one
        # (audit #21).
        if isinstance(exc, Exception):
            print(f"Neural-voice setup failed: {exc}", file=sys.stderr)
        if existed:
            if isinstance(exc, Exception):
                print("Kept your existing neural voices in {0}.".format(
                    paths.KOKORO_VENV), file=sys.stderr)
        else:
            try:
                kp.uninstall_kokoro()
            except Exception as rm_exc:  # noqa: BLE001 - never mask the cause
                print("Could not remove the half-built {0}: {1}".format(
                    paths.KOKORO_VENV, rm_exc), file=sys.stderr)
        restore()
        if isinstance(exc, Exception):
            return 1
        raise
    rc = 1
    try:
        rc = install()  # re-wires the daemon onto the venv python (neural_enabled() now True)
    finally:
        # install() can fail before its own stop and sentinel-clearing
        # finally (no Python, no plugin tree): undo the stop above.
        if rc != 0:
            restore()
    if rc == 0 and kp.neural_healthy(str(paths.APP_DIR)):
        print("Neural voices ready. Pick one with: sonara voice af_heart")
    return rc


def _cmd_voices_uninstall(_args) -> int:
    """Remove the Kokoro venv and revert the daemon to system Python.

    STOP the daemon first (#23): the kokoro venv IS the daemon's interpreter
    (pythonw locks Scripts/); deleting it live raised a raw PermissionError and
    left a half-deleted venv that still read as provisioned."""
    from sonara import kokoro_provision as kp
    # Check before deleting the venv: if install() then refused (H3), the
    # scheduled task kept pointing at the deleted venv's pythonw.
    if _resolve_plugin_root() is None:
        _print_no_plugin_root()
        return 1
    stop_sonara()
    try:
        kp.uninstall_kokoro()
        rc = install()  # neural_enabled() now False -> reverts to resolve_python()
    finally:
        # The stop above is this command's own: never leave Sonara off because
        # install() returned before it reached its own sentinel cleanup (E6).
        try:
            os.remove(str(paths.STOPPED_SENTINEL_PATH))
        except OSError:
            pass
    if rc == 0:
        print("Neural voices removed; reverted to the system voice.")
    return rc


def _cmd_cleanup(_args) -> int:
    """Remove the removed Chatterbox engine's leftovers (#134): its venv, model
    cache and smoke-test files. voices/chatterbox, the user's own recorded
    clips, is never touched.

    The daemon is stopped first: a still-running Chatterbox worker locks files
    in the venv, and deleting it live failed partway. It is started again only
    if it was running, and an earlier explicit shutdown stays in place."""
    from sonara import chatterbox_legacy as cl
    found = cl.leftovers()
    if not found:
        print("Nothing to clean up: no Chatterbox leftovers in {0}.".format(
            paths.SONARA_DIR))
        return 0
    total = sum(size for _p, size in found)
    restore = _stopped_state_restorer()
    if not stop_sonara():
        # stop_sonara already wrote the sentinel and ended the task: undo
        # that, or a daemon that later exits would never come back.
        restore()
        print("Sonara did not stop, so nothing was removed (a running worker "
              "would lock the files). Run 'sonara shutdown', then try again.",
              file=sys.stderr)
        return 1
    removed, failed = cl.remove_leftovers()
    for p in removed:
        print("Removed {0}".format(p))
    for p, exc in failed:
        print("Could not remove {0}: {1}".format(p, exc), file=sys.stderr)
    restore()
    if failed:
        return 1
    print("Freed {0}. Your voice clips in {1} were kept.".format(
        cl.format_size(total), paths.CHATTERBOX_VOICES_DIR))
    return 0


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
