"""`sonara doctor`: the health-check rows, from the platform backend plus the
portable checks (version, Sonara dir, daemon socket, hooks, keymap, summary command,
neural voices, Chatterbox leftovers, Python, the deployed copy)."""
from __future__ import annotations

import os
import time

from sonara import config_schema, install_record, keymap, paths
from sonara import platform as sonara_platform
from sonara.install import deps
from sonara.platform.base import DOCTOR_WARN  # noqa: F401 - the CLI reads it here
from sonara.protocol import MsgType, PROTOCOL_VERSION


def doctor() -> list:
    """Return a list of (check, ok, detail) health-check tuples."""
    plat = sonara_platform.get_platform()
    results = []

    # Platform-specific rows supplied by the OS backend (Windows:
    # schtasks/Task/pythonw/neural voice/...).
    results.extend(plat.supervisor.doctor_rows())
    # Hotkey diagnostics (Windows: collisions + UIPI/elevation).
    results.extend(plat.hotkey.doctor_rows())

    # Neutral rows (portable, keep inline).
    results.append(version_row())
    try:
        paths.ensure_sonara_dir()
        writable = os.access(str(paths.SONARA_DIR), os.W_OK)
        results.append(("SONARA_DIR writable", writable,
                        str(paths.SONARA_DIR) if writable
                        else f"{paths.SONARA_DIR} is not writable"))
    except Exception as exc:  # noqa: BLE001
        results.append(("SONARA_DIR writable", False, f"error: {exc}"))

    try:
        from sonara import client
        reply = client.send({"v": PROTOCOL_VERSION, "type": MsgType.PING},
                            expect_reply=True)
        ok = bool(reply) and reply.get("ok") is True
        results.append(("daemon socket", ok,
                        "reachable" if ok else "no ok reply from daemon"))
    except Exception as exc:  # noqa: BLE001
        results.append(("daemon socket", False,
                        f"not reachable: {exc} (run 'sonara start')"))

    results.append(plat.supervisor.hooks_doctor_row())

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
        results.append(neural_voices_row())
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
        py = deps.resolve_python()
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


def version_row() -> tuple:
    """This copy's version, plus the one install.json recorded when they
    differ (a deploy that did not take). Informational: never a FAIL."""
    import sonara
    mine = sonara.__version__
    try:
        installed = (install_record.read() or {}).get("plugin_version")
    except Exception:  # noqa: BLE001 - doctor must never raise
        installed = None
    if installed and installed != mine:
        return ("version", True, "{0} (install.json records {1})".format(
            mine, installed))
    return ("version", True, mine)


def neural_voices_row() -> tuple:
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
