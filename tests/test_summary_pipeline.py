"""daemon/summary/pipeline.py on its own (#141): the summary pipeline works
with only the state it is given, no SpeechDaemon around it."""
from __future__ import annotations

import threading

from sonara.daemon.core import SharedState
from sonara.daemon.summary.pipeline import SummaryPipeline
from sonara.daemon.summary.reorder import DigestReorderBuffer
from sonara.history import SessionHistory
from sonara.queue import SpeechItem
from sonara.router import Router
from sonara.sessions import SessionManager


class _Store:
    def __init__(self):
        self.saved = {}

    def set(self, session, text):
        self.saved[session] = text


def _pipeline(summary_mode=True):
    config = {"summary_mode": summary_mode}
    sessions = SessionManager()
    sessions.set_foreground("fg")
    router = Router(sessions, minqueue=lambda: 1,
                    announce_text=lambda folder, replay=False: folder)
    lock = threading.Lock()
    wake = threading.Event()
    history = SessionHistory()
    shared = SharedState()
    pending_heard = {}
    enqueued = []
    ids = iter(range(1, 1000))

    def enqueue(session, kind, text, is_decision, entry=None):
        enqueued.append((session, kind, text))

    p = SummaryPipeline(config, lock, wake, router, sessions, history,
                        _Store(), DigestReorderBuffer(lock=lock), shared,
                        pending_heard, enqueue=enqueue,
                        replay=lambda *a, **k: None, earcon=lambda kind: None)
    p.schedule_settle = lambda session, gen: None   # drive settle_fire by hand
    p.start_thread = lambda *a, **k: None           # no real summarizer
    p.schedule_hold_release = lambda *a: None
    p.schedule_digest_watchdog = lambda *a: None
    return p, router, history, shared, pending_heard, enqueued, ids


def _question(ids, session="fg"):
    return SpeechItem(id=next(ids), session=session, kind="choice",
                      text="Deploy now?", is_decision=True)


def test_decision_outside_summary_mode_is_enqueued_at_once():
    p, router, _h, _s, _ph, _e, ids = _pipeline(summary_mode=False)
    q = _question(ids)
    p.on_decision("fg", q)
    assert router.channel("fg").items == [q]
    assert "fg" not in p.pending_decision


def test_decision_in_summary_mode_waits_for_the_settle_window():
    p, router, _h, _s, _ph, _e, ids = _pipeline()
    q = _question(ids)
    p.on_decision("fg", q)
    assert p.pending_decision["fg"] == [q]
    assert "fg" in p.settle_pending and "fg" in p.busy_sessions()
    p.settle_fire("fg", p.settle_gen["fg"])          # no lead-in prose
    assert router.channel("fg").items == [q]
    assert not p.busy_sessions()


def test_long_turn_dispatches_a_digest_held_question_follows_it():
    p, router, history, shared, _ph, enqueued, ids = _pipeline()
    history.record("fg", "prose", "Context sentence. " * 30)
    q = _question(ids)
    p.on_decision("fg", q)
    p.settle_fire("fg", p.settle_gen["fg"])
    assert p.inflight == {"fg": 1}
    token, held = p.held_decision["fg"]
    assert held == [q]
    p.summarize_fn = lambda text, **kw: "Short recap."
    p.worker("fg", 0, "ignored", token, leadin=True)
    assert enqueued == [("fg", "summary", "Short recap.")]
    assert shared.last_digest_text["fg"] == "Short recap."
    assert router.channel("fg").items == [q]          # question after context
    assert not p.inflight and not p.held_decision


def test_cancel_drops_held_work_and_bumps_the_epochs():
    p, _router, history, _s, pending_heard, _e, ids = _pipeline()
    history.record("fg", "prose", "Context sentence. " * 30)
    q = _question(ids)
    pending_heard[q.id] = object()
    p.on_decision("fg", q)
    p.settle_fire("fg", p.settle_gen["fg"])
    settle_gen = p.settle_gen["fg"]
    p.cancel("fg")
    assert p.cancel_gen["fg"] == 1
    assert p.settle_gen["fg"] == settle_gen + 1
    assert not p.busy_sessions()
    assert "fg" not in p.voiced_upto
    assert q.id not in pending_heard


def test_caught_up_kills_the_inflight_digest_and_advances_the_marker():
    p, _router, history, _s, _ph, _e, _ids = _pipeline()
    history.record("fg", "prose", "Context sentence. " * 30)
    p.on_turn_done("fg")
    p.settle_fire("fg", p.settle_gen["fg"])
    assert p.inflight == {"fg": 1}
    later = history.record("fg", "prose", "Said before the answer.")
    assert p.caught_up("fg") is True
    assert p.cancel_gen["fg"] == 1 and not p.inflight
    assert p.voiced_upto["fg"] is later
    assert p.caught_up("fg") is False                 # nothing left to drop
