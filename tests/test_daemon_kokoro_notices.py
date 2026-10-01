"""Kokoro status the daemon speaks (E11, E12): the one-time 'downloading'
notice, and no bogus 'Kokoro unavailable' on an install without Kokoro."""
from sonara import kokoro
from tests.daemon_helpers import make_daemon


def _drain(daemon, n=4):
    for _ in range(n):
        daemon._speak_loop_once()


def test_cue_voice_is_native_when_kokoro_is_not_installed(monkeypatch):
    """E12: the default cue voice is af_heart; without Kokoro it must not
    route cues to an engine that is not there."""
    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    daemon, _q, _sp, _s, _c = make_daemon(foreground="fg")
    daemon.config["cue_voice"] = "af_heart"
    assert daemon._cues.cue_voice() is None


def test_cue_voice_is_kept_when_kokoro_is_installed(monkeypatch):
    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    daemon, _q, _sp, _s, _c = make_daemon(foreground="fg")
    daemon.config["cue_voice"] = "af_heart"
    assert daemon._cues.cue_voice() == "af_heart"


def test_native_cue_voice_is_kept_without_kokoro(monkeypatch):
    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    daemon, _q, _sp, _s, _c = make_daemon(foreground="fg")
    daemon.config["cue_voice"] = "Microsoft Zira"
    assert daemon._cues.cue_voice() == "Microsoft Zira"


def test_download_notice_is_spoken_once(monkeypatch):
    daemon, _q, speaker, _s, _c = make_daemon(foreground="fg")
    kokoro.pop_download_notice()
    kokoro._set_download_notice()
    _drain(daemon)
    kokoro._set_download_notice()          # a second download in the same run
    _drain(daemon)
    said = [t for t in speaker.spoken if "downloading" in t.lower()]
    assert len(said) == 1
    assert "windows voice" in said[0].lower()


def test_no_unavailable_notice_on_a_default_install(monkeypatch):
    """E12 end to end: nothing armed, nothing said."""
    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    kokoro.pop_fallback_notice()
    daemon, _q, speaker, _s, _c = make_daemon(foreground="fg")
    daemon._cues.speak("fg", "Muted.", exempt_mute=True)
    _drain(daemon)
    assert not any("kokoro" in t.lower() for t in speaker.spoken)
