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
