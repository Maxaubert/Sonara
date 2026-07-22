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





def test_rapid_duck_level_changes_supersede_pending_cues(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    from sonara.router import CONTROL
    ch = daemon.router.channel(CONTROL)
    _msg(daemon, type=MsgType.SET_DUCK_LEVEL, level=20)
    _msg(daemon, type=MsgType.SET_DUCK_LEVEL, level=60)
    pending = [i for i in ch.items[ch.cursor:] if "percent" in i.text]
    assert [i.text for i in pending] == ["Duck level 60 percent."]




def test_set_volume_never_speaks_a_cue(monkeypatch):
    # User decision: no spoken confirmation, ever. The instant session-volume
    # change is its own feedback and the slider shows the number on screen.
    from sonara.queue import SpeechItem
    from sonara.router import CONTROL
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    monkeypatch.setattr(daemon, "_apply_volume", lambda v: None)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=50)          # idle
    daemon._current_item = SpeechItem(id=1, session="fg", kind="summary",
                                      text="a digest being spoken",
                                      is_decision=False)
    _msg(daemon, type=MsgType.SET_VOLUME, volume=150)         # mid-speech
    ch = daemon.router.channel(CONTROL)
    assert not any("percent" in i.text for i in ch.items)
    assert speaker.cancels == 0                               # nothing cut
    assert config["volume"] == 150
