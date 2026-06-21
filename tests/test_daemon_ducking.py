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
