"""Ducking must engage at PLAYBACK start, not synthesis start. The daemon no
longer ducks before speaker.speak(); it passes _maybe_duck as speak's on_play
callback, which the TTS backend fires right before audio begins. With a slow
neural voice, ducking-at-synthesis held other apps' audio down for 5+ seconds
of silence (observed live with paragraph-length summary digests)."""
from sonara.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _seed_item(daemon, text="Hello there."):
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=1, session="fg", kind="prose", text=text,
                         is_decision=False))
    ch.turn_done = True


def test_duck_fires_via_on_play_not_before_speak():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "duck"
    _seed_item(daemon)
    ducked_at_entry = []

    original_speak = speaker.speak

    def probing_speak(text, cancel_epoch=None, on_play=None):
        # the daemon must NOT have ducked yet when speak() is entered
        ducked_at_entry.append(daemon.ducker.is_ducked())
        return original_speak(text, cancel_epoch=cancel_epoch, on_play=on_play)

    speaker.speak = probing_speak
    daemon._speak_loop_once()
    assert ducked_at_entry == [False]          # no duck during "synthesis"
    assert daemon.ducker.duck_calls           # ...but on_play ducked for playback


def test_no_duck_when_audio_control_off():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "off"
    _seed_item(daemon)
    daemon._speak_loop_once()
    assert daemon.ducker.duck_calls == []
