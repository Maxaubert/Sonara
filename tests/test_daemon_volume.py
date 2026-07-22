"""SET_VOLUME daemon handler: clamp, persist, platform apply, spoken cue."""
from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(daemon, **kw):
    kw.setdefault("v", PROTOCOL_VERSION)
    return daemon.handle_message(kw)


def test_set_volume_clamps_persists_and_applies(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    applied = []
    monkeypatch.setattr(daemon, "_apply_volume", lambda v: applied.append(v))
    _msg(daemon, type=MsgType.SET_VOLUME, volume=150)
    assert config["volume"] == 150
    assert applied == [150]
    _msg(daemon, type=MsgType.SET_VOLUME, volume=999)
    assert config["volume"] == 200
    _msg(daemon, type=MsgType.SET_VOLUME, volume="junk")
    assert config["volume"] == 200                 # unchanged


def test_set_volume_speaks_confirmation(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    monkeypatch.setattr(daemon, "_apply_volume", lambda v: None)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=150)
    from sonara.router import CONTROL
    ch = daemon.router.channel(CONTROL)
    assert any("150 percent" in i.text for i in ch.items)


def test_set_volume_reaches_platform_gain(monkeypatch):
    # The whole-branch review found _apply_volume calling a nonexistent
    # method on the backend instance - swallowed by its except, so the
    # feature was a silent no-op live. This test crosses the real seam:
    # daemon -> get_platform().tts.set_volume -> module gain state. It does
    # NOT monkeypatch _apply_volume, unlike the tests above.
    import sonara.platform as platform_mod
    from sonara.platform.windows.tts import WinTtsBackend
    from sonara.platform.windows import tts as tts_mod

    class _StubPlatform:
        def __init__(self):
            self.tts = WinTtsBackend()

    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    monkeypatch.setattr(platform_mod, "get_platform", lambda: _StubPlatform())
    tts_mod.set_volume(100)                # known baseline
    _msg(daemon, type=MsgType.SET_VOLUME, volume=150)
    assert tts_mod.get_volume() == 150
    tts_mod.set_volume(100)                # restore for other tests


def test_rapid_volume_changes_supersede_pending_cues(monkeypatch):
    # Dragging the slider fires many changes; without coalescing every value
    # was queued and read out ("volume 20, 25, 30, ..."). Only the LATEST
    # pending volume cue may remain.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    monkeypatch.setattr(daemon, "_apply_volume", lambda v: None)
    from sonara.router import CONTROL
    ch = daemon.router.channel(CONTROL)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=50)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=75)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=150)
    pending = [i for i in ch.items[ch.cursor:] if "percent" in i.text]
    assert [i.text for i in pending] == ["Volume 150 percent."]


def test_volume_cue_does_not_supersede_other_cues(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    monkeypatch.setattr(daemon, "_apply_volume", lambda v: None)
    from sonara.router import CONTROL
    ch = daemon.router.channel(CONTROL)
    daemon._speak_cue(None, "Muted.", exempt_mute=True, pause_exempt=True)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=50)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=150)
    texts = [i.text for i in ch.items[ch.cursor:]]
    assert "Muted." in texts                          # unrelated cue survives
    assert texts.count("Volume 150 percent.") == 1
    assert "Volume 50 percent." not in texts


def test_new_volume_cue_cuts_the_one_being_spoken(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    monkeypatch.setattr(daemon, "_apply_volume", lambda v: None)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=50)
    from sonara.router import CONTROL
    ch = daemon.router.channel(CONTROL)
    daemon._current_item = ch.next()                  # "Volume 50 percent." speaking
    before = speaker.cancels
    _msg(daemon, type=MsgType.SET_VOLUME, volume=150)
    assert speaker.cancels == before + 1              # stale value cut short


def test_rapid_duck_level_changes_supersede_pending_cues(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    from sonara.router import CONTROL
    ch = daemon.router.channel(CONTROL)
    _msg(daemon, type=MsgType.SET_DUCK_LEVEL, level=20)
    _msg(daemon, type=MsgType.SET_DUCK_LEVEL, level=60)
    pending = [i for i in ch.items[ch.cursor:] if "percent" in i.text]
    assert [i.text for i in pending] == ["Duck level 60 percent."]


def test_no_cue_while_content_is_speaking(monkeypatch):
    # Mid-speech the instant session volume IS the feedback; a spoken
    # confirmation minutes later was pure noise (user report). Only an idle
    # daemon (or one speaking a stale volume cue) confirms aloud.
    from sonara.queue import SpeechItem
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    monkeypatch.setattr(daemon, "_apply_volume", lambda v: None)
    daemon._current_item = SpeechItem(id=1, session="fg", kind="summary",
                                      text="a digest being spoken",
                                      is_decision=False)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=150)
    from sonara.router import CONTROL
    ch = daemon.router.channel(CONTROL)
    assert not any("percent" in i.text for i in ch.items[ch.cursor:])
    assert config["volume"] == 150                    # change still applied
