"""Windows media pause: pause OTHER apps' playing media while Sonara speaks (#92).

Uses the System Media Transport Controls (SMTC) API via winrt to pause exactly
the media sessions that are Playing, then resume exactly those. Mirrors
ducking.py's contract: all winrt imports are lazy, every public method swallows
errors and never raises, and a state file makes a mid-pause crash recoverable.

Only real media (apps that register SMTC transport controls) is affected. Audio
that is not an SMTC session (game SFX, calls, notifications) is left untouched.
"""
from __future__ import annotations

import json
import os
import threading

from sonara.paths import SONARA_DIR, ensure_sonara_dir

_PAUSE_STATE = SONARA_DIR / "pause_state.json"


# --- WinRT seams (patched wholesale in tests; never imported off Windows) ------

def _run_async(op):
    """Block on a WinRT IAsyncOperation from a sync context. Best-effort."""
    import asyncio

    async def _await():
        return await op

    loop = asyncio.new_event_loop()
    try:
        return loop.run_until_complete(_await())
    finally:
        loop.close()


def _session_manager():
    from winrt.windows.media.control import (
        GlobalSystemMediaTransportControlsSessionManager as Manager,
    )
    return _run_async(Manager.request_async())


def _sessions(mgr):
    return list(mgr.get_sessions())


def _is_playing(session) -> bool:
    from winrt.windows.media.control import (
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    )
    info = session.get_playback_info()
    return info.playback_status == Status.PLAYING


def _pause(session) -> None:
    _run_async(session.try_pause_async())


def _play(session) -> None:
    _run_async(session.try_play_async())


def _app_id(session) -> str:
    return session.source_app_user_model_id


# --- Public API ----------------------------------------------------------------

class MediaPauser:
    """Pause every OTHER app's currently-playing media session, then resume it."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._paused_ids: list[str] = []
        self._paused = False

    def is_paused(self) -> bool:
        with self._lock:
            return self._paused

    def pause(self) -> None:
        with self._lock:
            if self._paused:
                return
            try:
                mgr = _session_manager()
                ids = []
                for s in _sessions(mgr):
                    try:
                        if _is_playing(s):
                            _pause(s)
                            ids.append(_app_id(s))
                    except Exception:  # noqa: BLE001 - skip a bad session, keep the rest
                        continue
                self._paused_ids = ids
                self._paused = True
                _write_state(ids)
            except Exception:  # noqa: BLE001 - best-effort; never break speech
                pass

    def resume(self) -> None:
        with self._lock:
            try:
                wanted = set(self._paused_ids)
                if wanted:
                    try:
                        mgr = _session_manager()
                        for s in _sessions(mgr):
                            try:
                                if _app_id(s) in wanted:
                                    _play(s)
                            except Exception:  # noqa: BLE001 - one bad session must not block the rest
                                continue
                    except Exception:  # noqa: BLE001
                        pass
            finally:
                self._paused_ids = []
                self._paused = False
                _clear_state()


class NullPauser:
    """No-op pauser: non-Windows, missing winrt, or the daemon default until the
    real backend is injected. Mirrors NullDucker."""

    def is_paused(self) -> bool:
        return False

    def pause(self) -> None:
        pass

    def resume(self) -> None:
        pass


def _write_state(ids) -> None:
    try:
        ensure_sonara_dir()
        with open(_PAUSE_STATE, "w", encoding="utf-8") as f:
            json.dump({"apps": list(ids)}, f)
    except Exception:  # noqa: BLE001
        pass


def _clear_state() -> None:
    try:
        os.unlink(_PAUSE_STATE)
    except OSError:
        pass


def resume_from_state_file() -> None:
    """Daemon-startup crash sweep: if a prior daemon died mid-pause, resume any
    live SMTC session whose app id matches a recorded entry, then delete the
    file. Best-effort; never raises."""
    try:
        with open(_PAUSE_STATE, "r", encoding="utf-8") as f:
            data = json.load(f)
    except Exception:  # noqa: BLE001 - no/unreadable state -> nothing to resume
        return
    try:
        wanted = set(data.get("apps", []))
        if wanted:
            mgr = _session_manager()
            for s in _sessions(mgr):
                try:
                    if _app_id(s) in wanted:
                        _play(s)
                except Exception:  # noqa: BLE001
                    continue
    except Exception:  # noqa: BLE001
        pass
    finally:
        _clear_state()
