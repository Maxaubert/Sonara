from tests.daemon_helpers import make_daemon
from sonara.platform.windows.ducking import NullDucker


def test_daemon_defaults_to_null_ducker_when_none_passed():
    from sonara.daemon import SpeechDaemon
    from tests.daemon_helpers import FakeSpeaker
    from sonara.sessions import SessionManager
    from sonara.config import DEFAULTS
    d = SpeechDaemon(FakeSpeaker(), SessionManager(), dict(DEFAULTS))
    assert isinstance(d.ducker, NullDucker)


def test_make_daemon_injects_a_fake_ducker():
    daemon, *_ = make_daemon(foreground="fg")
    assert hasattr(daemon.ducker, "duck_calls")
    assert daemon.ducker.duck_calls == [] and daemon.ducker.restore_calls == 0


from sonara.protocol import MsgType, PROTOCOL_VERSION
from sonara.config import DEFAULTS


def test_config_defaults_have_audio_control_off_and_duck_level_20():
    assert DEFAULTS["audio_control"] is False
    assert DEFAULTS["duck_level"] == 20


def test_set_audio_control_on_persists_and_cues(monkeypatch):
    saved = {}
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: saved.update(c))
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL,
                           "enabled": True})
    assert daemon.config["audio_control"] is True
    assert saved.get("audio_control") is True
    # a spoken confirmation cue was queued on the CONTROL channel
    from sonara.router import CONTROL
    texts = [it.text for it in daemon.router.channel(CONTROL).items]
    assert any("Audio control on" in t for t in texts)


def test_set_audio_control_off_while_ducked_restores_now(monkeypatch):
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: None)
    daemon, *_ = make_daemon(foreground="fg")
    daemon.config["audio_control"] = True
    daemon.ducker._ducked = True               # pretend currently ducked
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL,
                           "enabled": False})
    assert daemon.config["audio_control"] is False
    assert daemon.ducker.restore_calls == 1


def test_set_duck_level_clamps_and_persists(monkeypatch):
    saved = {}
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: saved.update(c))
    daemon, *_ = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL,
                           "level": 150})
    assert daemon.config["duck_level"] == 100   # clamped
    assert saved.get("duck_level") == 100
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL,
                           "level": -5})
    assert daemon.config["duck_level"] == 0
    assert saved.get("duck_level") == 0


def test_set_audio_control_missing_enabled_is_noop(monkeypatch):
    saved = {}
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: saved.update(c))
    daemon, *_ = make_daemon(foreground="fg")
    # malformed message with no "enabled" key must not persist or flip the default
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_AUDIO_CONTROL})
    assert daemon.config["audio_control"] is False   # unchanged from default
    assert saved == {}                               # save_config never called


def test_set_duck_level_reapplies_when_ducked(monkeypatch):
    monkeypatch.setattr("sonara.daemon.save_config", lambda c: None)
    daemon, *_ = make_daemon(foreground="fg")
    daemon.ducker._ducked = True
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_DUCK_LEVEL,
                           "level": 35})
    assert daemon.ducker.restore_calls == 1                 # restored then re-ducked
    assert daemon.ducker.duck_calls[-1][1] == 35
