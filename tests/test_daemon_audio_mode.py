from sonara.daemon import SpeechDaemon
from sonara.sessions import SessionManager
from sonara.config import DEFAULTS
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
