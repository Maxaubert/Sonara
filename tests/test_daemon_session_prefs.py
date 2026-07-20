"""SET_SESSION_PREF / FORGET_SESSION daemon handlers."""
from sonara.daemon import SpeechDaemon
from sonara.sessions import SessionManager
from sonara.session_prefs import SessionPrefs
from tests.daemon_helpers import FakeSpeaker


def make_daemon(prefs=None):
    d = SpeechDaemon(FakeSpeaker(), SessionManager(), {"minqueue": 1},
                     prefs=prefs or SessionPrefs())
    return d


def test_set_pref_persists_via_store():
    p = SessionPrefs()
    d = make_daemon(prefs=p)
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "name", "value": "alpha"})
    assert p.name("s1") == "alpha"


def test_set_muted_applies_to_live_channel():
    d = make_daemon()
    ch = d.router.channel("s1")
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "muted", "value": True})
    assert ch.muted is True
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "muted", "value": False})
    assert ch.muted is False


def test_muting_the_speaking_session_cancels_current():
    d = make_daemon()
    d.router.channel("s1")

    class Cur:
        session = "s1"
    d._current_item = Cur()
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "muted", "value": True})
    assert d.speaker.cancels          # FakeSpeaker.cancel() increments cancels


def test_bad_key_or_session_is_ignored():
    p = SessionPrefs()
    d = make_daemon(prefs=p)
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "rate", "value": 300})
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": 7, "key": "name", "value": "x"})
    assert p.get("s1") == {}


def test_forget_session_clears_everything():
    p = SessionPrefs()
    d = make_daemon(prefs=p)
    d.sessions.register("s1", cwd="/x/proj")
    p.set("s1", "name", "alpha")
    d.router.channel("s1")
    d.handle_message({"v": 1, "type": "forget_session", "session": "s1"})
    assert p.get("s1") == {}
    assert d.sessions.folder("s1") is None
    assert "s1" not in d.router.channels


def test_forget_refuses_foreground():
    d = make_daemon()
    d.sessions.set_foreground("s1", cwd="/x/proj")
    d.router.channel("s1")
    d.handle_message({"v": 1, "type": "forget_session", "session": "s1"})
    assert "s1" in d.router.channels


def test_forget_session_clears_await_choice():
    """Forget targets exactly the stale sessions that died WITHOUT SessionEnd
    (#101): a leftover _await_choice entry suppresses permission chimes
    daemon-wide forever (global truthiness check), so it must be cleared too."""
    d = make_daemon()
    d.router.channel("s1")
    d._await_choice.add("s1")
    d.handle_message({"v": 1, "type": "forget_session", "session": "s1"})
    assert "s1" not in d._await_choice


def test_forget_session_clears_history():
    d = make_daemon()
    d.router.channel("s1")
    d.history.record("s1", "prose", "Hello there.")
    assert d.history.unheard("s1") != []
    d.handle_message({"v": 1, "type": "forget_session", "session": "s1"})
    assert d.history.unheard("s1") == []


def test_session_bearing_message_touches_last_seen():
    d = make_daemon()
    d.handle_message({"v": 1, "type": "set_foreground", "session": "s1"})
    assert d.sessions.last_seen("s1") is not None


def test_page_mutations_do_not_touch_last_seen():
    # managing a stale session from the settings page must not make it look
    # recently active, or naming an old row would bump it back into the list
    d = make_daemon()
    d.handle_message({"v": 1, "type": "set_session_pref",
                      "session": "s1", "key": "name", "value": "alpha"})
    d.handle_message({"v": 1, "type": "forget_session", "session": "s2"})
    assert d.sessions.last_seen("s1") is None
    assert d.sessions.last_seen("s2") is None
