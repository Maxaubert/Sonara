"""Spoken control cues: one-off confirmations ("Muted.", "Rate 210.", ...)
on the reserved CONTROL channel, the fast cue voice they speak in (#60), the
Kokoro prewarm, and the once-per-run Kokoro fallback and download notices.

Cues reslice the CONTROL channel and allocate item ids, which the speak loop
also does under the daemon lock, so callers hold the daemon lock. Known
exception: the speak loop calls maybe_announce_kokoro_fallback off-lock
(pre-existing, tracked in #155).

Cues keeps the daemon's config, router, speaker and session prefs by
reference, so the daemon must not rebind those attributes after
construction."""
from __future__ import annotations

import threading

from sonara import config_schema
from sonara.queue import SpeechItem


class Cues:
    """Owns the cue path. Given the daemon's shared state explicitly:
    *alloc_id* hands out item ids, *current_item* returns the item being
    spoken right now, *wake* is the speak loop's wake event."""

    def __init__(self, config, router, speaker, session_prefs, alloc_id,
                 current_item, wake) -> None:
        self._config = config
        self._router = router
        self._speaker = speaker
        self._session_prefs = session_prefs
        self._alloc_id = alloc_id
        self._current_item = current_item
        self._wake = wake
        self._kokoro_download_announced = False
        self._kokoro_fallback_announced = False

    def speak(self, session, text: str, exempt_mute: bool = False,
              pause_exempt: bool = False, cue_key=None, voice=None) -> None:
        """Speak a one-off confirmation/feedback cue (pause/mute/repeat/...).
        These ALWAYS go to the reserved CONTROL channel, which the router serves
        ahead of every session on `pending() > 0` -- bypassing the minqueue gate. A
        session channel is gated by `ready()` (minqueue items / turn_done), so a cue
        placed there during a live stream would sit unplayed and then burst out when
        the turn flushed; CONTROL makes the cue immediate regardless of stream state.
        The *session* arg is accepted for call-site clarity but no longer routes.

        *cue_key* coalesces slider spam: a keyed cue removes every pending cue
        with the same key and cuts one mid-speech, so dragging a slider speaks
        only the final value instead of the whole stacked sweep.

        *voice* speaks this one cue in that voice instead of the cue voice
        (a settings-page preview, M6)."""
        from sonara.router import CONTROL
        ch = self._router.channel(CONTROL)
        if ch.caught_up():
            ch.wipe()                      # control cues don't replay; keep it small
        elif cue_key is not None:
            ch.remove_pending(lambda it: it.cue_key == cue_key)
        if cue_key is not None:
            cur = self._current_item()
            if cur is not None and getattr(cur, "cue_key", None) == cue_key:
                self._speaker.cancel()     # stale value mid-utterance: cut it
        item = SpeechItem(id=self._alloc_id(), session=CONTROL, kind="prose",
                          text=text, is_decision=False, mute_exempt=exempt_mute,
                          pause_exempt=pause_exempt, cue_key=cue_key,
                          voice=voice or None)
        # APPEND, do not cursor-insert: CONTROL is already served ahead of every
        # session, and inserting at the cursor made STACKED cues play LIFO --
        # the user heard state confirmations newest-first (deep audit #25).
        ch.append(item)
        self._wake.set()

    def cue_voice(self):
        """The voice cues speak in (#60): config cue_voice (default af_heart,
        the warm-Kokoro pick -- ~0.3s per cue once loaded, far nicer than the
        native David/Zira). Unset maps to None = the platform's native
        voice."""
        v = self._config.get("cue_voice")
        if not v:
            return None
        try:
            from sonara import kokoro
            if kokoro.is_kokoro_voice(v) and not kokoro.is_installed():
                # E12: the default af_heart on an install without Kokoro.
                # Speak cues natively instead of failing over every time.
                return None
        except Exception:  # noqa: BLE001 - a cue must never fail on the check
            pass
        return v

    def cue_voice_override(self, item) -> dict:
        """speaker.speak kwargs for *item* (#60). Control feedback and
        session-change announcements speak through an always-fast voice
        (warm Kokoro by default, native Windows as floor) instead of the
        configured voice, so "Muted." never waits on a slow synthesis.
        Config fast_cues (default on) disables."""
        from sonara.router import CONTROL
        if item.session == CONTROL and getattr(item, "voice", None):
            return {"voice": item.voice}         # a voice preview (M6)
        if (config_schema.get(self._config, "fast_cues")
                and (item.session == CONTROL or item.kind == "session_change")):
            return {"voice": self.cue_voice()}
        return {}

    def voice_override(self, item) -> dict:
        """speaker.speak kwargs for *item*: the fast-cue voice for control
        feedback and session-change announcements (#60), else the session's
        voice pref, else {} (the global default voice)."""
        kw = self.cue_voice_override(item)
        if kw:
            return kw
        v = self._session_prefs.voice(item.session)
        return {"voice": v} if v else {}

    def maybe_prewarm(self) -> None:
        """Pre-load the Kokoro engine when cues route to a Kokoro voice (#60):
        the first cue after daemon start otherwise pays the ~3s engine load.
        Best-effort, background, never blocks or breaks the caller."""
        try:
            from sonara import kokoro
            if not (config_schema.get(self._config, "fast_cues")
                    and kokoro.is_kokoro_voice(self.cue_voice())
                    and kokoro.is_installed()):
                return
        except Exception:  # noqa: BLE001 - optional engine; never break startup
            return

        def _warm():
            try:
                from sonara.platform import get_platform
                get_platform().tts.prewarm(config_schema.get(self._config, "rate"))
            except Exception:  # noqa: BLE001 - warming is best-effort
                pass
        threading.Thread(target=_warm, name="sonara-kokoro-warm", daemon=True).start()

    def maybe_announce_kokoro_fallback(self) -> None:
        """Speak the pending Kokoro fallback notice, if any, exactly once per
        daemon run (#29): a dead engine is announced instead of producing
        unexplained error noise. The one-time model download (M2/E11, #53)
        is announced the same way, so the Windows voice standing in meanwhile
        is explained."""
        try:
            from sonara import kokoro
            downloading = kokoro.pop_download_notice()
        except Exception:  # noqa: BLE001 - never let the notice check wedge the loop
            downloading = False
        if downloading and not self._kokoro_download_announced:
            self._kokoro_download_announced = True
            self.speak(None, "Downloading the neural voice. Using the "
                       "Windows voice until it is ready.", exempt_mute=True)
        if self._kokoro_fallback_announced:
            return
        try:
            from sonara import kokoro
            reason = kokoro.pop_fallback_notice()
        except Exception:  # noqa: BLE001 - never let the notice check wedge the loop
            return
        if reason:
            self._kokoro_fallback_announced = True
            self.speak(None, "Kokoro unavailable, using Windows voice.",
                       exempt_mute=True)
