"""Settings-page voice preview (#34). M6: it plays through the speak loop's
own queue, after whatever is being spoken, so it can never cut live speech
(winsound has one channel) nor have the cut utterance marked heard."""
from sonara.router import CONTROL
from tests.daemon_helpers import make_daemon


def _drain(daemon, n=4):
    for _ in range(n):
        daemon._speak_loop_once()


def test_preview_voice_speaks_sample_with_named_voice():
    daemon, _q, speaker, _s, _c = make_daemon()
    assert daemon.preview_voice("af_bella") is True
    _drain(daemon)
    i = next(i for i, t in enumerate(speaker.spoken) if "af_bella" in t)
    assert speaker.speak_voices[i] == "af_bella"
    assert daemon.config["voice"] != "af_bella"       # config untouched


def test_preview_never_cuts_live_speech():
    """M6: the preview queues on CONTROL instead of playing over the speaker."""
    daemon, _q, speaker, _s, _c = make_daemon()
    live = object()
    daemon._current_item = live                  # a live utterance is playing
    assert daemon.preview_voice("af_bella") is True
    assert speaker.cancels == 0                  # nothing was cut
    pending = [it.text for it in daemon.router.channel(CONTROL).items]
    assert any("af_bella" in t for t in pending)


def test_a_new_preview_replaces_a_pending_one():
    daemon, _q, speaker, _s, _c = make_daemon()
    daemon.preview_voice("af_bella")
    daemon.preview_voice("af_heart")
    _drain(daemon)
    said = [t for t in speaker.spoken if "speaking for Sonara" in t]
    assert len(said) == 1 and "af_heart" in said[0]


def test_preview_plays_while_muted():
    """The user asked for it explicitly: mute does not swallow it."""
    daemon, _q, speaker, _s, _c = make_daemon()
    daemon._mute_level = 1
    daemon.preview_voice("af_bella")
    _drain(daemon)
    assert any("af_bella" in t for t in speaker.spoken)


def test_preview_rejects_an_empty_voice():
    daemon, *_ = make_daemon()
    assert daemon.preview_voice("") is False
