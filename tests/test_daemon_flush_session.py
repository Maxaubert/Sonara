"""Ctrl+Alt+Down = flush to end: silence EVERYTHING queued or in flight across
ALL sessions and go idle, non-destructively (skipped items stay unheard so
catch-up recovers them). Global since #107: the old per-engaged-session flush
left other sessions' landed or reorder-parked digests holding the floor, so a
flush chimed success and a handoff started reading seconds later anyway."""
from sonara.protocol import MsgType, PROTOCOL_VERSION
from sonara.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _flush(daemon, session="fg"):
    daemon.handle_message({"type": MsgType.FLUSH_SESSION, "session": session})


def _prose(session, text, idx):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": session,
            "delta": text, "index": idx, "final": True}


def test_flush_advances_cursor_to_end_and_cancels_current():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=1, session="fg", kind="prose", text="a", is_decision=False))
    ch.append(SpeechItem(id=2, session="fg", kind="prose", text="b", is_decision=False))
    daemon._current_item = SpeechItem(id=3, session="fg", kind="prose",
                                      text="cur", is_decision=False)
    _flush(daemon)
    assert ch.cursor == len(ch.items)        # nothing pending
    assert ch.pending() == 0
    assert speaker.cancels == 1              # current utterance cut
    assert speaker.earcons[-1] == "nav"


def test_flush_cuts_another_sessions_current_audio_too():
    # Global flush (#107): the key means "silence, now" - the in-progress
    # utterance is cut no matter which session owns it.
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=1, session="fg", kind="prose", text="a", is_decision=False))
    daemon._current_item = SpeechItem(id=9, session="other", kind="prose",
                                      text="x", is_decision=False)
    _flush(daemon)
    assert ch.cursor == len(ch.items)        # fg drained
    assert speaker.cancels == 1              # other session's audio cut too


def test_flush_clears_pending_decision_flag():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=1, session="fg", kind="permission", text="ok?",
                         is_decision=True))
    assert ch.has_decision is True
    _flush(daemon)
    assert ch.has_decision is False


def test_flush_second_press_after_cutting_edges_not_moved():
    # "Go to end" is top/bottom: a rapid second press (before the speak loop has
    # cleared _current_item) must give the barrier chime, not another "nav". The
    # cut clears _current_item so the re-press sees nothing left to cut (issue #11).
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=1, session="fg", kind="summary", text="digest", is_decision=False))
    ch.cursor = 1                              # already reading it (cursor past it)
    daemon._current_item = SpeechItem(id=1, session="fg", kind="summary",
                                      text="digest", is_decision=False)
    _flush(daemon)                             # first press: cut the current digest
    assert speaker.earcons[-1] == "nav"
    assert speaker.cancels == 1
    _flush(daemon)                             # rapid second press
    assert speaker.earcons[-1] == "nav_edge"  # nothing left to cut -> barrier
    assert speaker.cancels == 1               # did not cancel again


def test_flush_with_nothing_pending_is_a_safe_edge_no_op():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    _flush(daemon)
    assert daemon.router.channel("fg").cursor == 0
    assert speaker.earcons[-1] == "nav_edge"


def test_flush_with_no_engaged_session_edges():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground=None)
    daemon.handle_message({"type": MsgType.FLUSH_SESSION, "session": "x"})
    assert speaker.earcons[-1] == "nav_edge"


def test_flushed_items_stay_recoverable_via_catch_up():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message(_prose("fg", "One. ", 0))
    daemon.handle_message(_prose("fg", "Two. ", 1))
    _flush(daemon)
    assert daemon.router.channel("fg").pending() == 0        # flushed
    daemon.handle_message({"type": MsgType.CATCH_UP, "session": "fg"})
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert "One." in texts and "Two." in texts               # catch-up brought them back


def test_flush_drains_every_sessions_backlog_in_one_press():
    # The soft-lock scenario (#107): session A is reading, session B's digest
    # already landed on B's channel. One flush must silence BOTH, or B takes
    # the floor seconds later and a re-press in the gap hits an "empty" queue.
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="a")
    cha = daemon.router.channel("a")
    chb = daemon.router.channel("b")
    daemon._current_item = SpeechItem(id=1, session="a", kind="summary",
                                      text="reading now", is_decision=False)
    chb.append(SpeechItem(id=2, session="b", kind="summary",
                          text="landed digest", is_decision=False))
    chb.turn_done = True
    _flush(daemon)
    assert speaker.cancels == 1              # a's audio cut
    assert chb.pending() == 0                # b's landed digest skipped
    assert speaker.earcons[-1] == "nav"


def test_flush_lands_parked_reorder_digests_dead():
    # A COMPLETED digest parked behind an earlier reorder slot is not
    # inflight anymore, so the per-session kill cannot see it; flush must
    # land the parked slot dead or it speaks when the earlier slot releases.
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    ran = []
    daemon._digest_seq_next = 2
    daemon._digest_seq_serve = 0
    daemon._digest_parked[1] = lambda: ran.append(1)   # completed, waiting on seq 0
    _flush(daemon)
    assert speaker.earcons[-1] == "nav"      # killing it counts as success
    daemon._land_digest(0, None)             # the earlier slot finally releases
    assert ran == []                         # the parked digest never speaks


def test_flush_drops_stale_handoff_alert_and_armed_announce():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon._pending_preamble = ("b", "Session changed: b.")
    daemon.router._pending_announce = "b"
    _flush(daemon)
    assert daemon._pending_preamble is None
    assert daemon.router._pending_announce is None


def test_flush_during_settle_window_kills_the_upcoming_digest_with_success():
    # Pressing flush in the 600ms settle gap (before the digest dispatches)
    # must count as success (nav chime), not an edge no-op: it just silenced
    # the digest that was about to generate and speak.
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.config["summary_mode"] = True
    daemon._arm_settle("fg")
    assert "fg" in daemon._settle_pending
    _flush(daemon)
    assert "fg" not in daemon._settle_pending
    assert speaker.earcons[-1] == "nav"
