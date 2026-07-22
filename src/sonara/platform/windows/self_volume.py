"""Sonara's OWN audio-session volume: the instant half of speech volume.

Digital gain (tts._scale_wav) cannot touch audio that is already synthesized
or already playing, so lowering the volume used to wait out the current chunk
or the whole Kokoro utterance (user report). The Windows per-app session
volume applies to the daemon's winsound output INSTANTLY, mid-playback, but
only attenuates (100 percent is the ceiling): it carries the 25..100 range
while sample gain carries the boost above 100.

Lazy pycaw/COM imports and best-effort everywhere, mirroring ducking.py: a
volume failure must never break or delay speech. The daemon process has no
audio session until winsound first plays, so apply_self_volume reports
success/failure and the playback path retries until it sticks.
"""
from __future__ import annotations

import os


def _sessions():
    """Seam for tests: ducking's all-devices session enumeration."""
    from sonara.platform.windows.ducking import _all_sessions
    return _all_sessions()


def apply_self_volume(percent) -> bool:
    """Set this process's audio-session master volume to min(percent, 100)/100.

    Returns True when at least one own-process session was set; False when the
    process has no audio session yet (nothing played since start) or COM is
    unavailable. Never raises."""
    try:
        target = max(0, min(100, int(percent))) / 100.0
        pid = os.getpid()
        hit = False
        for s in _sessions():
            try:
                if s.ProcessId == pid and s.SimpleAudioVolume is not None:
                    s.SimpleAudioVolume.SetMasterVolume(target, None)
                    hit = True
            except Exception:  # noqa: BLE001 - skip a bad session, keep the rest
                continue
        return hit
    except Exception:  # noqa: BLE001 - volume must never break speech
        return False
