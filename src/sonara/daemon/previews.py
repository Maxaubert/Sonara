"""Settings-page voice previews on the daemon side: the background builder
that renders missing preview files (#38, the files themselves are
sonara.previews) and the live preview a click queues on the CONTROL channel
(#34, M6)."""
from __future__ import annotations

import threading


def start_preview_builder(delay_s: float = 15.0):
    """Render missing voice-preview files in the background (#38). Delayed
    so daemon startup (prewarm, first speech) is never contended; every
    failure is contained -- previews are a convenience, not a duty.
    Returns the thread (tests join it)."""
    def _run():
        try:
            import time
            time.sleep(delay_s)
            from sonara import previews
            from sonara.webui import _installed_voices
            made = previews.ensure_all(
                _installed_voices(),
                log=lambda m: print("[previews] " + m, flush=True))
            if made:
                print("[previews] rendered {0} preview file(s)".format(made),
                      flush=True)
        except Exception:  # noqa: BLE001 - preview building must never bite
            pass
    t = threading.Thread(target=_run, name="sonara-previews", daemon=True)
    t.start()
    return t


def preview_voice(lock, cues, voice: str) -> bool:
    """Speak a short sample in *voice* WITHOUT changing config (settings
    page, #34). It queues on the CONTROL channel like any cue (M6): it
    plays after the utterance in progress, never over it. Playing it on
    its own thread cut live speech (winsound has one channel) and the cut
    utterance was still marked heard. A newer preview replaces a pending
    or playing one; mute and pause do not swallow it, the user asked."""
    if not voice:
        return False
    text = "This is {0} speaking for Sonara.".format(voice)
    # HTTP requests run on their own threads: Cues.speak reslices CONTROL
    # and allocates an id, which the speak loop does under the lock too.
    with lock:
        cues.speak(None, text, exempt_mute=True, pause_exempt=True,
                   cue_key="voice_preview", voice=str(voice))
    return True
