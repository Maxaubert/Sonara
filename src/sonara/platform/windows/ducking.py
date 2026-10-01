"""Windows audio ducking: lower OTHER apps' volume while Sonara speaks.

Per-app volume via pycaw (Core Audio session API). All pycaw/comtypes imports are
lazy so this module imports anywhere (tests, non-Windows). Best-effort: every
public method swallows pycaw/COM errors and never raises, so a failure to duck can
never break or delay speech.
"""
from __future__ import annotations

import json
import os
import sys
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
    """Lower every other app's audio session to a target level, then restore.

    Every session is handled on its own, so one failing session (e.g. a virtual
    device invalidated mid-enumeration) never strands the ones already lowered:
    whatever was lowered is recorded, in memory and in the state file, and a
    restore that fails keeps its record for a retry (#130)."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._saved = []          # list[(session, record)] lowered by the current duck
        self._pending = []        # records whose restore failed; retried on next restore
        self._ducked = False

    def is_ducked(self) -> bool:
        with self._lock:
            return self._ducked

    def recover(self) -> None:
        """Startup crash sweep (PlatformBackend.recover_audio)."""
        restore_from_state_file()

    def duck(self, exclude_pids, level: int) -> None:
        with self._lock:
            if self._ducked:
                return
            target = max(0, min(100, int(level))) / 100.0
            saved, enumerated = [], False
            try:
                sessions = _all_sessions()
                enumerated = True
                for s in sessions:
                    try:
                        vol = s.SimpleAudioVolume
                        name = _session_name(s)
                        if (vol is None or s.ProcessId in exclude_pids
                                or name.lower() in _NEVER_DUCK):
                            continue
                        original = vol.GetMasterVolume()
                        if original <= target + 0.005:
                            # Already at or below the duck level: nothing to lower,
                            # and recording it would save a ducked level as the
                            # "original" (the stuck-at-30% bug).
                            continue
                        vol.SetMasterVolume(target, None)
                        saved.append((s, {"pid": s.ProcessId, "name": name,
                                          "original": original}))
                    except Exception as exc:  # noqa: BLE001 - one bad session must not block the rest
                        _log(f"session error while ducking: {exc!r}")
            except Exception as exc:  # noqa: BLE001 - best-effort; never break speech
                _log(f"cannot enumerate audio sessions: {exc!r}")
            finally:
                self._saved = saved
                # Enumeration failed and nothing was lowered: stay un-ducked so the
                # next call retries.
                self._ducked = enumerated or bool(saved)
                # A pending record superseded by a fresh duck of the same app is
                # dropped: the fresh "original" reflects any change made since.
                fresh = {(r["pid"], r["name"]) for _, r in saved}
                self._pending = [p for p in self._pending
                                 if (p.get("pid"), p.get("name")) not in fresh]
                records = self._pending + [r for _, r in saved]
                if records:
                    _write_state(records)
                if saved:
                    _log("lowered " + _names(r for _, r in saved))

    def restore(self) -> None:
        with self._lock:
            failed = list(self._pending)
            try:
                for s, rec in self._saved:
                    try:
                        s.SimpleAudioVolume.SetMasterVolume(rec["original"], None)
                    except Exception:  # noqa: BLE001 - retried below by a fresh lookup
                        failed.append(rec)
                if failed:
                    failed = _restore_records(failed)
            except Exception as exc:  # noqa: BLE001 - keep `failed` for the next attempt
                _log(f"cannot enumerate audio sessions for restore: {exc!r}")
            finally:
                self._saved = []
                self._ducked = False
                self._pending = failed
                if failed:
                    _write_state(failed)
                    _log("restore failed for " + _names(failed))
                else:
                    _clear_state()


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


def _log(msg: str) -> None:
    print(f"[duck] {msg}", file=sys.stderr, flush=True)


def _names(records) -> str:
    return ", ".join(r.get("name") or str(r.get("pid")) for r in records)


def _restore_records(records):
    """Restore recorded sessions by a FRESH enumeration, matched by pid, then by
    process name (the saved session object may be stale, or the app restarted).
    Returns the records that still could not be restored. A record with no live
    session is dropped: its process is gone. Raises only if enumeration fails."""
    by_pid = {r["pid"]: r for r in records if "pid" in r}
    by_name = {r["name"]: r for r in records if r.get("name")}
    done, failed = set(), []
    for s in _all_sessions():
        name = _session_name(s)
        rec = by_pid.get(s.ProcessId)
        if (rec is not None and rec.get("name") and name
                and rec["name"].lower() != name.lower()):
            rec = None   # L-duck-pid: a reused pid belongs to another app now
        if rec is None:
            rec = by_name.get(name)
        if rec is None or id(rec) in done:
            continue
        try:
            s.SimpleAudioVolume.SetMasterVolume(rec["original"], None)
            done.add(id(rec))
        except Exception:  # noqa: BLE001
            failed.append(rec)
    return [r for r in failed if id(r) not in done]


def restore_from_state_file() -> None:
    """Daemon-startup crash sweep: if a prior daemon died mid-duck (or a restore
    failed), restore any live session whose pid or process name matches a
    recorded entry. Entries that still fail stay in the file for the next sweep;
    if enumeration itself fails the file is kept untouched. Never raises."""
    try:
        with open(_DUCK_STATE, "r", encoding="utf-8") as f:
            records = json.load(f).get("sessions", [])
    except Exception:  # noqa: BLE001 - no/unreadable state -> nothing to restore
        return
    try:
        remaining = _restore_records(records)
    except Exception as exc:  # noqa: BLE001
        _log(f"startup restore could not enumerate sessions: {exc!r}")
        return
    if remaining:
        _write_state(remaining)
        _log("startup restore failed for " + _names(remaining))
    else:
        _clear_state()
