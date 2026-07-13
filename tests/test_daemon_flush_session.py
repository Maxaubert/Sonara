"""Ctrl+Alt+Down = flush to end: skip ALL pending items for the engaged session
and go idle, non-destructively (skipped items stay unheard so catch-up recovers
them). Models JUMP_DECISION but advances the cursor to the very end."""
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


def test_flush_does_not_cancel_when_current_is_another_session():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=1, session="fg", kind="prose", text="a", is_decision=False))
    daemon._current_item = SpeechItem(id=9, session="other", kind="prose",
                                      text="x", is_decision=False)
    _flush(daemon)
    assert ch.cursor == len(ch.items)        # fg still drained
    assert speaker.cancels == 0              # other session's audio untouched


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
