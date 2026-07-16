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
