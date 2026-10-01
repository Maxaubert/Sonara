"""`sonara install` and `sonara uninstall`: resolve the interpreter, provision
the speech engine, stop Sonara, deploy the runtime copy and let the platform
backend wire autostart, hooks and the launcher; uninstall reverses it while
keeping the user's settings."""
from __future__ import annotations

import os
import shutil

from sonara import install_record, keymap, paths
from sonara import platform as sonara_platform
from sonara.install import app_copy, deps, service


def install_runtime(sup, python: str, py_ver: str, plugin_root: str):
    """install() steps 2-5, run while Sonara is stopped: copy the runtime,
    keymap, install record, then the OS autostart + hooks + launcher. Returns
    APP_DIR, or None after printing why it failed. Never raises an ordinary
    error: install() must get the chance to clear the stop sentinel."""
    # 2. Copy the package into the stable APP_DIR (decouples the long-lived
    #    daemon from the version-pinned marketplace cache; see spec §3.B).
    try:
        app_dir = app_copy.copy_app(plugin_root)
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
        plugin_version = app_copy.read_plugin_version(plugin_root)
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
    plat = sonara_platform.get_platform()
    sup = plat.supervisor

    # 1. Resolve the best Python >= 3.9 (FATAL if none).
    python = deps.daemon_python(sup)
    if python is None:
        print("No suitable Python >= 3.9 found. Install Python 3.9+ "
              "(python.org) and re-run: sonara install")
        return 1
    ver = sup._probe_python_version(python)
    py_ver = "{0}.{1}".format(*ver) if ver else "3.9"
    print(f"Using interpreter: {python} (Python {py_ver})")

    # 1a. Find the plugin tree BEFORE changing anything: from the deployed
    #     copy there may be none, and stopping first left Sonara off (H3).
    plugin_root = app_copy.resolve_plugin_root()
    if plugin_root is None:
        app_copy.print_no_plugin_root()
        return 1

    # 1b. Ensure the Windows speech engine (PyWinRT) is installed in that Python.
    #     Claude Code does NOT install a plugin's optional Python deps, so without
    #     this a fresh install is silently voiceless. install() owns it.
    speech_ok = deps.ensure_speech_deps(python)

    # 1c. STOP Sonara before touching APP_DIR (#23): the scheduled task's
    #     working directory sits INSIDE the tree being replaced, so mutating it
    #     under a running daemon/supervisor half-deleted the app (the documented
    #     'gutted app' failure). The sentinel also blocks a hook event from
    #     lazily respawning the daemon mid-install. It is cleared whether the
    #     steps below succeed or fail (E6): a failed install that left it in
    #     place kept Sonara off with no cue.
    service.stop_sonara(sup)
    try:
        app_dir = install_runtime(sup, python, py_ver, plugin_root)
    finally:
        service.clear_stop_sentinel()
    if app_dir is None:
        return 1

    # 6. Global hotkeys. Windows hotkeys run in-process and are started by the
    #    daemon (deferred to M3, announced in post_install_notes).
    plat.hotkey.install()

    # 7. Voice check. Only meaningful once the speech engine is present; otherwise
    #    surface the "add a voice" path so N/KN and bare-Windows users aren't stuck.
    if speech_ok:
        try:
            voice = plat.tts.best_voice()
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


def uninstall() -> int:
    """Remove Sonara's OS autostart/hooks/launcher (via the platform backend)
    plus the shared runtime artifacts, PRESERVING config.json + keymap.json."""
    plat = sonara_platform.get_platform()
    sup = plat.supervisor
    # STOP everything FIRST (#23): the old order deleted the task definition and
    # files while the supervisor/daemon kept running (and kept respawning from a
    # deleted install).
    service.stop_sonara(sup)
    sup.uninstall()
    try:
        plat.hotkey.uninstall()
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
        # Legacy files from the removed hotkeyd; older installs still wrote
        # hotkeyd.resolved.json, so uninstall keeps sweeping them.
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
