"""Per-session voice override resolution (#session-manager)."""
from sonara.session_prefs import SessionPrefs
from sonara.router import CONTROL
from tests.test_daemon_session_prefs import make_daemon


class Item:
    def __init__(self, session, kind="prose"):
        self.session = session
        self.kind = kind


def test_session_voice_pref_wins_over_default():
    p = SessionPrefs()
    p.set("s1", "voice", "af_nicole")
    d = make_daemon(prefs=p)
    assert d._voice_override(Item("s1")) == {"voice": "af_nicole"}


def test_no_pref_means_no_override():
    d = make_daemon()
    assert d._voice_override(Item("s1")) == {}


def test_cue_override_beats_session_pref():
    p = SessionPrefs()
    p.set(CONTROL, "voice", "af_nicole")     # nonsensical but must not matter
    d = make_daemon(prefs=p)
    d.config["fast_cues"] = True
    kw = d._voice_override(Item(CONTROL))
    assert kw == {"voice": d._cue_voice()}
