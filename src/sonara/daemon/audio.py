"""Other apps' audio while Sonara speaks (#92): the audio mode (off, duck,
pause), the duck level and the pids spared from ducking, engaging at
playback start and restoring when idle, plus the speech volume. Handles
SET_AUDIO_MODE, SET_DUCK_LEVEL and SET_VOLUME.

Product rule: never leave other apps ducked or paused."""
from __future__ import annotations

import os

from sonara import config_schema
from sonara.protocol import MsgType


class AudioControl:
    """Owns the ducker and pauser. Given the daemon's shared state
    explicitly: *persist* saves the config, *cues* speaks confirmations,
    *cue_target* names the session a cue is said for, *wake* is the speak
    loop's wake event."""

    MESSAGES = (MsgType.SET_AUDIO_MODE, MsgType.SET_DUCK_LEVEL,
                MsgType.SET_VOLUME)

    def __init__(self, config, ducker, pauser, speaker, persist, cues,
                 cue_target, wake) -> None:
        self._config = config
        self.ducker = ducker
        self.pauser = pauser
        self._speaker = speaker
        self._persist = persist
        self._cues = cues
        self._cue_target = cue_target
        self._wake = wake

    def mode(self) -> str:
        return config_schema.current(self._config, "audio_mode")

    def duck_on(self) -> bool:
        return self.mode() == "duck"

    def duck_level(self) -> int:
        return config_schema.current(self._config, "duck_level")

    def duck_exclude_pids(self) -> "set[int]":
        pids = {os.getpid()}
        try:
            pids.update(self._speaker.earcon_pids())
        except AttributeError:
            pass
        return pids

    def apply_volume(self, percent) -> None:
        """Push the speech gain to the platform playback layer. Best-effort:
        tests and non-Windows runs have no platform backend."""
        try:
            from sonara.platform import get_platform
            get_platform().tts.set_volume(percent)
        except Exception:  # noqa: BLE001 - volume must never break the daemon
            pass

    def engage(self) -> None:
        mode = self.mode()
        if mode == "duck":
            if not self.ducker.is_ducked():
                self.ducker.duck(self.duck_exclude_pids(), self.duck_level())
        elif mode == "pause":
            if not self.pauser.is_paused():
                self.pauser.pause()

    def restore(self) -> None:
        # Disengage BOTH backends defensively: a mid-speech mode switch can leave
        # the other backend engaged, and idle must never leave media ducked OR paused.
        if self.ducker.is_ducked():
            self.ducker.restore()
        if self.pauser.is_paused():
            self.pauser.resume()

    def set_mode(self, mode: str) -> None:
        """Persist the audio behavior mode, disengage whatever backend was
        engaged (so a switch never leaves other apps ducked or paused), and
        speak the mode cue."""
        if mode not in config_schema.AUDIO_MODES:
            return
        self._config["audio_mode"] = mode
        self._persist()
        self.restore()
        target = self._cue_target()
        cue = {"off": "Audio off.", "duck": "Audio ducking.",
               "pause": "Media pause."}[mode]
        self._cues.speak(target, cue, exempt_mute=True, pause_exempt=True)
        self._wake.set()

    def handle(self, msg):
        """Apply one of MESSAGES. Caller holds the daemon lock. Returns None
        (no reply), like the daemon's other setters."""
        t = msg.get("type")
        if t == MsgType.SET_AUDIO_MODE:
            mode = config_schema.clean("audio_mode", msg.get("mode"))
            if mode is config_schema.INVALID:
                return None
            self.set_mode(mode)
            return None

        if t == MsgType.SET_DUCK_LEVEL:
            level = config_schema.clean("duck_level", msg.get("level"))
            if level is config_schema.INVALID:
                return None
            self._config["duck_level"] = level
            self._persist()
            if self.duck_on() and self.ducker.is_ducked():  # re-apply at the new level
                self.ducker.restore()
                self.ducker.duck(self.duck_exclude_pids(), level)
            target = self._cue_target()
            self._cues.speak(target, "Duck level {0} percent.".format(level),
                             exempt_mute=True, pause_exempt=True,
                             cue_key="duck_level")
            self._wake.set()
            return None

        if t == MsgType.SET_VOLUME:
            vol = config_schema.clean("volume", msg.get("volume"))
            if vol is config_schema.INVALID:
                return None
            self._config["volume"] = vol
            self._persist()
            self.apply_volume(vol)
            # No spoken confirmation, ever (user decision): the instant
            # session-volume change is its own feedback, and the slider is
            # the only surface, so the number is already on screen.
            self._wake.set()
            return None

        return None
