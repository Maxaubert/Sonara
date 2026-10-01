"""daemon/playback.py on its own (#141): the speak loop works with only the
state it is given, no SpeechDaemon around it."""
from __future__ import annotations

import threading

from sonara.daemon.core import SharedState
from sonara.daemon.playback import SpeakLoop
from sonara.queue import SpeechItem
from sonara.router import Router
from sonara.sessions import SessionManager
from tests.daemon_helpers import FakeSpeaker


class _Cues:
    def cue_voice_override(self, item):
        return {}

    def voice_override(self, item):
        return {}

    def cue_voice(self):
        return None

    def maybe_announce_kokoro_fallback(self):
        pass


class _Audio:
    def __init__(self):
        self.engaged = 0
        self.restored = 0

    def engage(self, still_wanted=None):
        if still_wanted is None or still_wanted():
            self.engaged += 1

    def restore(self):
        self.restored += 1


def _loop(muted=False):
    sessions = SessionManager()
    sessions.set_foreground("fg")
    router = Router(sessions, minqueue=lambda: 1,
                    announce_text=lambda folder, replay=False: folder)
    speaker = FakeSpeaker()
    audio = _Audio()
    shared = SharedState()
    pending_heard = {}
    earcons = []
    noted = []
    loop = SpeakLoop({"fast_cues": True}, threading.Lock(), threading.Event(),
                     threading.Event(), threading.Event(), router, speaker,
                     _Cues(), audio, shared, pending_heard,
                     muted=lambda: muted, earcon=earcons.append,
                     note_spoken=lambda item, c: noted.append((item.text, c)),
                     requeue_or_note=lambda item, c: False)
    loop.poll_interval = 0.0
    return loop, router, speaker, audio, shared, pending_heard, earcons, noted


def _put(router, text, item_id=1, mute_exempt=False):
    ch = router.channel("fg")
    ch.append(SpeechItem(id=item_id, session="fg", kind="prose", text=text,
                         is_decision=False, mute_exempt=mute_exempt))
    ch.turn_done = True


def test_speaks_the_next_item_engages_audio_and_notes_it():
    loop, router, speaker, audio, shared, _ph, _e, noted = _loop()
    _put(router, "hello")
    loop.run_once()
    assert speaker.spoken == ["hello"]
    assert audio.engaged == 1
    assert noted == [("hello", True)]
    assert shared.current_item is not None and shared.current_item.text == "hello"


def test_muted_drops_the_item_and_its_heard_marker():
    loop, router, speaker, _a, shared, pending_heard, _e, noted = _loop(muted=True)
    _put(router, "quiet please")
    pending_heard[1] = object()
    loop.run_once()
    assert speaker.spoken == [] and noted == []
    assert 1 not in pending_heard and shared.current_item is None


def test_a_failed_utterance_chimes_error_and_is_noted_unfinished():
    loop, router, speaker, _a, _s, _ph, earcons, noted = _loop()

    def boom(*a, **k):
        raise RuntimeError("synth failed")

    speaker.speak = boom
    _put(router, "broken")
    loop.run_once()
    assert earcons == ["error"]
    assert noted == [("broken", False)]


def test_drop_preamble_for_only_drops_that_sessions_alert():
    loop = _loop()[0]
    loop.pending_preamble = ("a", "Session changed: a.")
    loop.drop_preamble_for("b")
    assert loop.pending_preamble == ("a", "Session changed: a.")
    loop.drop_preamble_for("a")
    assert loop.pending_preamble is None
