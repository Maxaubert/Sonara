from sonara.daemon import SpeechDaemon
from sonara.sessions import SessionManager
from sonara.config import DEFAULTS
from sonara.queue import SpeechItem
from tests.daemon_helpers import make_daemon, FakePauser


def test_daemon_defaults_to_null_pauser():
    from sonara.platform.windows.pausing import NullPauser
    cfg = {k: v for k, v in DEFAULTS.items()}
    d = SpeechDaemon(object(), SessionManager(), cfg)   # no pauser passed
    assert isinstance(d.pauser, NullPauser)


def test_make_daemon_injects_fake_pauser():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert isinstance(daemon.pauser, FakePauser)
    assert daemon.pauser.pause_calls == 0


def _seed_item(daemon, text="Hello there.", session="fg"):
    ch = daemon.router.channel(session)
    ch.append(SpeechItem(id=1, session=session, kind="prose", text=text,
                         is_decision=False))
    ch.turn_done = True


def test_off_mode_engages_neither_backend():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "off"
    _seed_item(daemon)
    daemon._speak_loop_once()
    assert daemon.ducker.duck_calls == []
    assert daemon.pauser.pause_calls == 0


def test_duck_mode_ducks_at_playback():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "duck"
    _seed_item(daemon)
    daemon._speak_loop_once()
    assert daemon.ducker.duck_calls
    assert daemon.pauser.pause_calls == 0


def test_pause_mode_pauses_at_playback():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "pause"
    _seed_item(daemon)
    daemon._speak_loop_once()
    assert daemon.pauser.pause_calls == 1
    assert daemon.ducker.duck_calls == []


def test_pause_mode_resumes_at_global_idle():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "pause"
    _seed_item(daemon)
    daemon._speak_loop_once()                     # speaks -> paused
    assert daemon.pauser.is_paused() is True
    daemon._speak_loop_once()                     # nothing left -> idle restore
    assert daemon.pauser.resume_calls == 1
    assert daemon.pauser.is_paused() is False


def test_session_change_announcement_engages_neither():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    config["audio_mode"] = "pause"
    daemon.router._last_active = "a"
    _seed_item(daemon, text="The digest body.", session="b")
    daemon.router._replay_authorized.add("b")
    daemon._speak_loop_once()                     # the announcement
    assert daemon.pauser.pause_calls == 0
    assert daemon.ducker.duck_calls == []
    daemon._speak_loop_once()                     # the content
    assert daemon.pauser.pause_calls == 1


from sonara.protocol import MsgType, PROTOCOL_VERSION


def _msg(daemon, **kw):
    kw.setdefault("v", PROTOCOL_VERSION)
    return daemon.handle_message(kw)


def test_set_audio_mode_persists_and_cues():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _msg(daemon, type=MsgType.SET_AUDIO_MODE, mode="pause")
    assert config["audio_mode"] == "pause"
    # cues are queued on the reserved CONTROL channel, not spoken synchronously
    # (mirrors test_set_audio_control_on_persists_and_cues in test_daemon_ducking.py)
    from sonara.router import CONTROL
    texts = [it.text for it in daemon.router.channel(CONTROL).items]
    assert any("Media pause." in t for t in texts)


def test_set_audio_mode_disengages_previous_backend():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "duck"
    _seed_item(daemon)
    daemon._speak_loop_once()                     # ducked now
    assert daemon.ducker.is_ducked() is True
    _msg(daemon, type=MsgType.SET_AUDIO_MODE, mode="pause")
    assert daemon.ducker.is_ducked() is False     # old backend released on switch


def test_set_audio_mode_ignores_unknown_value():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "duck"
    _msg(daemon, type=MsgType.SET_AUDIO_MODE, mode="bogus")
    assert config["audio_mode"] == "duck"         # unchanged


def test_audio_control_shim_maps_to_mode():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _msg(daemon, type=MsgType.SET_AUDIO_CONTROL, enabled=True)
    assert config["audio_mode"] == "duck"
    _msg(daemon, type=MsgType.SET_AUDIO_CONTROL, enabled=False)
    assert config["audio_mode"] == "off"


def test_duck_level_reapplies_only_in_duck_mode():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    config["audio_mode"] = "pause"
    daemon.pauser.pause()                          # pretend paused
    _msg(daemon, type=MsgType.SET_DUCK_LEVEL, level=50)
    assert daemon.ducker.duck_calls == []         # not in duck mode -> no re-duck
