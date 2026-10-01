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


def test_preview_queues_its_cue_under_the_daemon_lock():
    """preview_voice runs on a webui HTTP thread. _cues.speak reslices the
    CONTROL channel and allocates an id, which the speak loop also does under
    self._lock, so the preview must take the lock too."""
    daemon, *_ = make_daemon()
    held = []
    daemon._cues.speak = lambda *a, **k: held.append(daemon._lock.locked())
    daemon.preview_voice("af_bella")
    assert held == [True]


def test_hotkey_failure_cue_is_queued_under_the_daemon_lock():
    """HotkeyController.reload and start run off the lock, so the failure
    cue they speak must take it (same race as the preview)."""
    daemon, *_ = make_daemon()
    held = []
    daemon._cues.speak = lambda *a, **k: held.append(daemon._lock.locked())
    try:
        raise RuntimeError("boom")
    except RuntimeError:
        daemon._hotkeys.failed("reload")
    daemon._hotkeys.announce_collisions([{"action": "mute"}])
    assert held == [True, True]
