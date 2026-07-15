"""#60: control cues speak via the instant Windows voice (voice=None override)
so "Muted." never waits out a cold Chatterbox model reload."""
from tests.daemon_helpers import make_daemon


def _drain(daemon, n=4):
    for _ in range(n):
        daemon._speak_loop_once()


def test_control_cue_speaks_with_cue_voice_override():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.config["voice"] = "linus"
    daemon.config["cue_voice"] = "af_heart"
    daemon._speak_cue("fg", "Muted.", exempt_mute=True)
    _drain(daemon, 2)
    assert "Muted." in speaker.spoken
    i = speaker.spoken.index("Muted.")
    assert speaker.speak_voices[i] == "af_heart"    # warm-Kokoro cue voice


def test_cue_voice_unset_falls_back_to_native_windows_voice():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.config["cue_voice"] = None
    daemon._speak_cue("fg", "Muted.", exempt_mute=True)
    _drain(daemon, 2)
    i = speaker.spoken.index("Muted.")
    assert speaker.speak_voices[i] is None          # None = best WinRT voice


def test_cue_voice_refuses_a_chatterbox_voice(monkeypatch):
    # A Chatterbox cue voice would reintroduce the cold-reload lag fast cues
    # exist to fix (#60): it maps to the native voice instead.
    import sonara.chatterbox as cb
    monkeypatch.setattr(cb, "is_chatterbox_voice", lambda v: v == "linus")
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.config["cue_voice"] = "linus"
    daemon._speak_cue("fg", "Muted.", exempt_mute=True)
    _drain(daemon, 2)
    i = speaker.spoken.index("Muted.")
    assert speaker.speak_voices[i] is None


def test_set_config_value_accepts_cue_voice():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._maybe_prewarm_cue_voice = lambda: None   # no engine spin-up in tests
    assert daemon.set_config_value("cue_voice", "af_nicole") is True
    assert daemon.config["cue_voice"] == "af_nicole"
    assert daemon.set_config_value("cue_voice", "   ") is False


def test_session_prose_keeps_configured_voice():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.config["voice"] = "linus"
    daemon.config["minqueue"] = 1
    daemon.handle_message({"v": 1, "type": "prose", "session": "fg",
                           "delta": "Regular content here. ", "index": 0,
                           "final": True})
    _drain(daemon)
    assert any("Regular content here." in t for t in speaker.spoken)
    i = next(i for i, t in enumerate(speaker.spoken) if "Regular content here." in t)
    assert speaker.speak_voices[i] == "__default__"  # no override passed


def test_fast_cues_off_keeps_configured_voice_for_cues():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.config["fast_cues"] = False
    daemon._speak_cue("fg", "Muted.", exempt_mute=True)
    _drain(daemon, 2)
    i = speaker.spoken.index("Muted.")
    assert speaker.speak_voices[i] == "__default__"


def test_session_change_announcement_gets_the_override():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    from sonara.daemon import SpeechItem
    item = SpeechItem(id=0, session="other", kind="session_change",
                      text="Session changed.", is_decision=False)
    assert daemon._cue_voice_override(item) == {"voice": "af_heart"}


def test_set_config_value_accepts_fast_cues():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon.set_config_value("fast_cues", False) is True
    assert daemon.config["fast_cues"] is False
    assert daemon.set_config_value("fast_cues", True) is True
    assert daemon.config["fast_cues"] is True
