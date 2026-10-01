"""`sonara voices install|uninstall`: provision or remove the Kokoro venv and
re-run install() so the daemon moves onto (or off) its interpreter."""
from __future__ import annotations

import os
import sys

from sonara import paths
from sonara.install import app_copy, installer, service


def install_voices() -> int:
    """Provision the Kokoro venv and re-wire the daemon onto it."""
    from sonara import kokoro_provision as kp
    # install() below refuses without a plugin tree (H3): check before the
    # ~316 MB download, not after it.
    if app_copy.resolve_plugin_root() is None:
        app_copy.print_no_plugin_root()
        return 1
    paths.ensure_sonara_dir()
    # E10: the daemon may be running on this very venv's pythonw, whose files
    # are then locked. Stop it first, and only ever remove a venv this run
    # created: a failed re-run or upgrade keeps the one that worked.
    existed = kp.neural_enabled()
    restore = service.stopped_state_restorer()
    if not service.stop_sonara():
        restore()
        service.print_not_stopped()
        return 1
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
            # provision() rebuilds a venv whose python cannot start, so a
            # failed rebuild may have removed (or half-removed) it already.
            if not isinstance(exc, Exception):
                pass  # Ctrl+C: re-raised below, no message
            elif os.path.exists(paths.kokoro_venv_python()):
                print("Kept your existing neural voices in {0}.".format(
                    paths.KOKORO_VENV), file=sys.stderr)
            else:
                print("The neural voices in {0} could not be rebuilt and are "
                      "gone. Run 'sonara voices uninstall' to switch back to "
                      "the built-in voices, or try 'sonara voices install' "
                      "again.".format(paths.KOKORO_VENV), file=sys.stderr)
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
        rc = installer.install()  # re-wires the daemon onto the venv python (neural_enabled() now True)
    finally:
        # install() can fail before its own stop and sentinel-clearing
        # finally (no Python, no plugin tree): undo the stop above.
        if rc != 0:
            restore()
    if rc == 0 and kp.neural_healthy(str(paths.APP_DIR)):
        print("Neural voices ready. Pick one with: sonara voice af_heart")
    return rc


def uninstall_voices() -> int:
    """Remove the Kokoro venv and revert the daemon to system Python.

    STOP the daemon first (#23): the kokoro venv IS the daemon's interpreter
    (pythonw locks Scripts/); deleting it live raised a raw PermissionError and
    left a half-deleted venv that still read as provisioned."""
    from sonara import kokoro_provision as kp
    # Check before deleting the venv: if install() then refused (H3), the
    # scheduled task kept pointing at the deleted venv's pythonw.
    if app_copy.resolve_plugin_root() is None:
        app_copy.print_no_plugin_root()
        return 1
    restore = service.stopped_state_restorer()
    if not service.stop_sonara():
        # Deleting under a live daemon hit its locked pythonw and left a
        # half-deleted venv plus a traceback (#166).
        restore()
        service.print_not_stopped()
        return 1
    try:
        kp.uninstall_kokoro()
        rc = installer.install()  # neural_enabled() now False -> reverts to resolve_python()
    finally:
        # The stop above is this command's own: never leave Sonara off because
        # install() returned before it reached its own sentinel cleanup (E6).
        service.clear_stop_sentinel()
    if rc == 0:
        print("Neural voices removed; reverted to the system voice.")
    return rc
