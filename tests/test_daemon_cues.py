"""daemon/cues.py on its own (#141): the cue path works with only the state
it is given, no SpeechDaemon around it."""
from __future__ import annotations

import threading

from sonara.daemon.cues import Cues
from sonara.queue import SpeechItem
from sonara.router import CONTROL, Router
from sonara.sessions import SessionManager
from tests.daemon_helpers import FakeSpeaker


class _Prefs:
    def voice(self, session):
        return "af_nicole" if session == "s1" else None


def _cues(config=None, current=None):
    ids = iter(range(1, 1000))
    router = Router(SessionManager(), minqueue=lambda: 1,
                    announce_text=lambda folder, replay=False: folder)
    speaker = FakeSpeaker()
    wake = threading.Event()
    cues = Cues(config if config is not None else {"fast_cues": True},
                router, speaker, _Prefs(), alloc_id=lambda: next(ids),
                current_item=lambda: current, wake=wake)
    return cues, router, speaker, wake


def test_speak_appends_to_control_and_wakes():
    cues, router, _speaker, wake = _cues()
    cues.speak("fg", "First.")
    cues.speak(None, "Second.", exempt_mute=True)
    items = router.channel(CONTROL).items
    assert [it.text for it in items] == ["First.", "Second."]
    assert items[1].mute_exempt is True
    assert wake.is_set()


def test_keyed_cue_replaces_pending_and_cuts_the_playing_one():
    playing = SpeechItem(id=99, session=CONTROL, kind="prose", text="Rate 200.",
                         is_decision=False, cue_key="rate")
    cues, router, speaker, _wake = _cues(current=playing)
    cues.speak(None, "Rate 205.", cue_key="rate")
    cues.speak(None, "Rate 210.", cue_key="rate")
    assert [it.text for it in router.channel(CONTROL).items] == ["Rate 210."]
    assert speaker.cancels == 2


def test_voice_override_prefers_cue_voice_then_session_pref():
    cues, *_ = _cues({"fast_cues": True, "cue_voice": "Microsoft Zira"})
    control = SpeechItem(id=1, session=CONTROL, kind="prose", text="x",
                         is_decision=False)
    content = SpeechItem(id=2, session="s1", kind="prose", text="y",
                         is_decision=False)
    assert cues.voice_override(control) == {"voice": "Microsoft Zira"}
    assert cues.voice_override(content) == {"voice": "af_nicole"}


def test_kokoro_fallback_notice_is_spoken_once(monkeypatch):
    from sonara import kokoro
    monkeypatch.setattr(kokoro, "pop_download_notice", lambda: False)
    monkeypatch.setattr(kokoro, "pop_fallback_notice", lambda: "engine died")
    cues, router, *_ = _cues()
    cues.maybe_announce_kokoro_fallback()
    cues.maybe_announce_kokoro_fallback()
    said = [it.text for it in router.channel(CONTROL).items]
    assert said == ["Kokoro unavailable, using Windows voice."]
