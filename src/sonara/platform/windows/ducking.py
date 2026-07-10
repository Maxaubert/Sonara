"""Windows audio ducking: lower OTHER apps' volume while Sonara speaks.

Per-app volume via pycaw (Core Audio session API). All pycaw/comtypes imports are
lazy so this module imports anywhere (tests, non-Windows). Best-effort: every
public method swallows pycaw/COM errors and never raises, so a failure to duck can
never break or delay speech.
"""
from __future__ import annotations

import json
import os
import threading

from sonara.paths import SONARA_DIR, ensure_sonara_dir

_DUCK_STATE = SONARA_DIR / "duck_state.json"


# Audio-engine / virtual-router processes that must NEVER be ducked: their session
# IS the aggregated output to the hardware, so lowering it drops the WHOLE mix
# (including Sonara's own speech), not a single app. audiodg.exe is the Windows
# audio engine; the rest are common per-app virtual-audio routers (SteelSeries
# Sonar, VoiceMeeter) whose process represents the final mix on the real device.
_NEVER_DUCK = frozenset({
    "audiodg.exe", "steelseriessonar.exe",
    "voicemeeter.exe", "voicemeeter8.exe", "voicemeeter8x64.exe",
})


def _all_sessions():
    """Active audio sessions across ALL active render devices, not just the default.

    Users with a virtual-audio mixer (SteelSeries Sonar, VoiceMeeter, ...) route
    different apps to different virtual output devices; the default-device-only
    pycaw `GetAllSessions()` misses the app actually playing media (e.g. a browser
    on a non-default 'Media' device). We enumerate every active render endpoint and
    collect its sessions as pycaw AudioSession objects (same shape GetAllSessions
    returns: .ProcessId / .Process / .SimpleAudioVolume). Lazy import; the test seam
    patches this. Raises if pycaw/COM is unavailable --- callers swallow it."""
    import comtypes
    from comtypes import CLSCTX_ALL
    from pycaw.pycaw import AudioSession
    from pycaw.api.mmdeviceapi import IMMDeviceEnumerator
    from pycaw.api.audiopolicy import IAudioSessionManager2, IAudioSessionControl2
    from pycaw.constants import CLSID_MMDeviceEnumerator

    _ERENDER, _DEVICE_STATE_ACTIVE = 0, 0x1
    enumerator = comtypes.CoCreateInstance(
        CLSID_MMDeviceEnumerator, IMMDeviceEnumerator, comtypes.CLSCTX_INPROC_SERVER)
    collection = enumerator.EnumAudioEndpoints(_ERENDER, _DEVICE_STATE_ACTIVE)
    sessions = []
    for i in range(collection.GetCount()):
        try:
            mgr = collection.Item(i).Activate(
                IAudioSessionManager2._iid_, CLSCTX_ALL, None)
            mgr2 = mgr.QueryInterface(IAudioSessionManager2)
            senum = mgr2.GetSessionEnumerator()
            for j in range(senum.GetCount()):
                try:
                    ctl2 = senum.GetSession(j).QueryInterface(IAudioSessionControl2)
                    sessions.append(AudioSession(ctl2))
                except Exception:  # noqa: BLE001 - skip a bad session, keep the rest
                    continue
        except Exception:  # noqa: BLE001 - skip a device we can't open
            continue
    return sessions


def _session_name(session) -> str:
    try:
        return session.Process.name() if session.Process else ""
    except Exception:  # noqa: BLE001
        return ""


class AudioDucker:
    """Lower every other app's audio session to a target level, then restore."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._saved = []          # list[(session, original_scalar)]
        self._ducked = False

    def is_ducked(self) -> bool:
        with self._lock:
            return self._ducked

    def duck(self, exclude_pids, level: int) -> None:
        with self._lock:
            if self._ducked:
                return
            try:
                target = max(0, min(100, int(level))) / 100.0
                saved, record = [], []
                for s in _all_sessions():
                    vol = s.SimpleAudioVolume
                    if (vol is None or s.ProcessId in exclude_pids
                            or _session_name(s).lower() in _NEVER_DUCK):
                        continue
                    original = vol.GetMasterVolume()
                    vol.SetMasterVolume(target, None)
                    saved.append((s, original))
                    record.append({"pid": s.ProcessId, "name": _session_name(s),
                                   "original": original})
                self._saved = saved
                self._ducked = True
                _write_state(record)
            except Exception:  # noqa: BLE001 - best-effort; never break speech
                pass

    def restore(self) -> None:
        with self._lock:
            try:
                for s, original in self._saved:
                    try:
                        s.SimpleAudioVolume.SetMasterVolume(original, None)
                    except Exception:  # noqa: BLE001 - one bad session must not block the rest
                        pass
            finally:
                self._saved = []
                self._ducked = False
                _clear_state()


class NullDucker:
    """No-op ducker: used on non-Windows, when pycaw is missing, or as the daemon
    default until the real backend ducker is injected."""

    def is_ducked(self) -> bool:
        return False

    def duck(self, exclude_pids, level: int) -> None:
        pass

    def restore(self) -> None:
        pass


def _write_state(record) -> None:
    try:
        ensure_sonara_dir()
        with open(_DUCK_STATE, "w", encoding="utf-8") as f:
            json.dump({"sessions": record}, f)
    except Exception:  # noqa: BLE001
        pass


def _clear_state() -> None:
    try:
        os.unlink(_DUCK_STATE)
    except OSError:
        pass


def restore_from_state_file() -> None:
    """Daemon-startup crash sweep: if a prior daemon died mid-duck, restore any
    live session whose pid or process name matches a recorded entry, then delete
    the file. Best-effort; never raises."""
    try:
        with open(_DUCK_STATE, "r", encoding="utf-8") as f:
            data = json.load(f)
    except Exception:  # noqa: BLE001 - no/unreadable state -> nothing to restore
        return
    try:
        by_pid, by_name = {}, {}
        for e in data.get("sessions", []):
            if "pid" in e:
                by_pid[e["pid"]] = e["original"]
            if e.get("name"):
                by_name[e["name"]] = e["original"]
        for s in _all_sessions():
            try:
                original = by_pid.get(s.ProcessId)
                if original is None:
                    original = by_name.get(_session_name(s))
                if original is not None:
                    s.SimpleAudioVolume.SetMasterVolume(original, None)
            except Exception:  # noqa: BLE001
                pass
    except Exception:  # noqa: BLE001
        pass
    finally:
        _clear_state()
