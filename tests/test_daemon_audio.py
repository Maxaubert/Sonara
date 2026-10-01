"""daemon/audio.py on its own (#141): AudioControl works with only the state
it is given, no SpeechDaemon around it."""
from __future__ import annotations

import threading

from sonara.daemon.audio import AudioControl
from sonara.protocol import MsgType
from tests.daemon_helpers import FakeDucker, FakePauser, FakeSpeaker


class _Cues:
    def __init__(self):
        self.said = []

    def speak(self, session, text, **kw):
        self.said.append((session, text))


def _audio(config):
    saved = []
    cues = _Cues()
    audio = AudioControl(config, FakeDucker(), FakePauser(), FakeSpeaker(),
                         persist=lambda: saved.append(dict(config)),
                         cues=cues, cue_target=lambda: "fg",
                         wake=threading.Event())
    return audio, saved, cues


def test_engage_follows_the_mode_and_restore_releases_both():
    audio, *_ = _audio({"audio_mode": "duck", "duck_level": 40})
    audio.engage()
    assert audio.ducker.is_ducked()
    audio.pauser.pause()                 # a mid-speech mode switch left it paused
    audio.restore()
    assert not audio.ducker.is_ducked() and not audio.pauser.is_paused()


def test_set_mode_persists_restores_and_confirms():
    config = {"audio_mode": "duck"}
    audio, saved, cues = _audio(config)
    audio.engage()
    audio.handle({"type": MsgType.SET_AUDIO_MODE, "mode": "pause"})
    assert config["audio_mode"] == "pause" and saved
    assert not audio.ducker.is_ducked()
    assert cues.said == [("fg", "Media pause.")]


def test_invalid_values_change_nothing():
    config = {"audio_mode": "off"}
    audio, saved, cues = _audio(config)
    audio.handle({"type": MsgType.SET_AUDIO_MODE, "mode": "loud"})
    audio.handle({"type": MsgType.SET_VOLUME, "volume": "max"})
    assert config == {"audio_mode": "off"}
    assert saved == [] and cues.said == []
