"""Message-cursor navigation (nav next/prev/first/last) over the current turn.
Each move cuts current speech, inserts replay items at the channel cursor, and
replays the target message forward; next/prev clamp at the ends (no wrap);
new content appends after the replay without interleaving."""
from sonari.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _drain_channel(daemon, session="fg"):
    """Read all pending items from the session's channel (advancing its cursor)."""
    ch = daemon.router.channel(session)
    items = []
    while ch.cursor < len(ch.items):
        items.append(ch.items[ch.cursor])
        ch.cursor += 1
    return items


def _seed(daemon):
    # Current turn = 3 messages: m0 (two sentences), m1, m2 (latest).
    h = daemon.history
    h.record("fg", "prose", "m0a"); h.record("fg", "prose", "m0b"); h.end_message("fg")
    h.record("fg", "prose", "m1"); h.end_message("fg")
    h.record("fg", "prose", "m2")


def _nav(daemon, to):
    daemon.handle_message({"type": "nav", "to": to, "session": "fg"})


def _prose(s, delta, idx, final):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s,
            "delta": delta, "index": idx, "final": final}


def test_repeat_reads_last_message_via_channel():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Hello. ", 0, True))
    daemon._speak_loop_once()                     # reads "Hello."
    speaker.spoken.clear()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.REPEAT})
    daemon._speak_loop_once()
    assert speaker.spoken == ["Hello."]


def test_prev_steps_back_one_message_then_plays_forward():
    # Seek-and-play: stepping back lands on the previous item AND reads every
    # later one, so playback continues instead of stopping after one item.
    daemon, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "prev")
    assert [s.text for s in _drain_channel(daemon)] == ["m1", "m2"]
    _nav(daemon, "prev")
    assert [s.text for s in _drain_channel(daemon)] == ["m0a", "m0b", "m1", "m2"]


def _start_reading(daemon, session, msg_id):
    """Simulate the speak loop currently reading a given message: point
    _current_item at an item whose history entry has that msg_id."""
    from sonari.queue import SpeechItem
    entry = daemon.history.entries_for_message(session, msg_id)[0]
    item = SpeechItem(id=9000 + msg_id, session=session, kind="prose",
                      text=entry.text, is_decision=False)
    daemon._pending_heard[item.id] = entry
    daemon._current_item = item


def test_nav_next_while_reading_earlier_message_anchors_on_it_not_latest():
    # Bug repro: reading m1 (msg 1) of a 3-message turn, pressing next must go to
    # m2 (the message AFTER what's playing), NOT jump to the latest with an edge
    # chime. Anchor = the message currently being read, not n-1.
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)                                    # messages 0, 1, 2 (m2 latest)
    _start_reading(daemon, "fg", 1)                  # currently hearing m1
    _nav(daemon, "next")
    assert speaker.earcons[-1] == "nav"              # moved, NOT nav_edge
    assert [s.text for s in _drain_channel(daemon)] == ["m2"]


def test_nav_prev_while_reading_anchors_on_current_not_latest():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _start_reading(daemon, "fg", 1)                  # hearing m1
    _nav(daemon, "prev")                             # -> m0 (before m1)
    assert speaker.earcons[-1] == "nav"
    assert [s.text for s in _drain_channel(daemon)] == ["m0a", "m0b", "m1", "m2"]


def test_nav_next_while_reading_the_latest_message_edges():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _start_reading(daemon, "fg", 2)                  # hearing the newest (m2)
    _nav(daemon, "next")                             # nothing after -> edge
    assert speaker.earcons[-1] == "nav_edge"


def test_prev_clamps_at_first():
    daemon, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    for _ in range(5):
        _nav(daemon, "prev")
    assert [s.text for s in _drain_channel(daemon)] == ["m0a", "m0b", "m1", "m2"]
    assert daemon._nav_cursor["fg"] == 0


def test_next_clamps_at_last_no_wrap():
    daemon, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "first"); _drain_channel(daemon)                 # cursor at m0
    _nav(daemon, "next"); assert [s.text for s in _drain_channel(daemon)] == ["m1", "m2"]
    _nav(daemon, "next"); assert [s.text for s in _drain_channel(daemon)] == ["m2"]
    _nav(daemon, "next")                                           # at last -> re-read m2
    assert [s.text for s in _drain_channel(daemon)] == ["m2"]     # never wrapped to 0
    # reaching the latest clears the cursor: "following live" again, not pinned
    assert daemon._nav_cursor.get("fg") is None


def test_first_and_last_jump():
    daemon, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "first")
    assert [s.text for s in _drain_channel(daemon)] == ["m0a", "m0b", "m1", "m2"]   # whole turn
    _nav(daemon, "last")
    assert [s.text for s in _drain_channel(daemon)] == ["m2"]


# --- nav chimes: 'nav' when a move lands, 'nav_edge' at a boundary/no-op -------

def test_nav_move_fires_nav_chime(speakerless=None):
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "first")          # cursor was at latest -> moving to first MOVES
    assert speaker.earcons[-1] == "nav"


def test_nav_at_edge_fires_nav_edge_chime():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "last")           # already at the latest -> no move -> edge
    assert speaker.earcons[-1] == "nav_edge"


def test_nav_prev_at_first_fires_nav_edge():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "first")          # move to first (nav)
    speaker.earcons.clear()
    _nav(daemon, "prev")           # already first -> edge
    assert speaker.earcons == ["nav_edge"]


def test_nav_with_no_history_fires_nav_edge():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")   # no messages seeded
    _nav(daemon, "next")
    assert speaker.earcons[-1] == "nav_edge"


def test_nav_with_no_foreground_fires_nav_edge():
    daemon, queue, speaker, *_ = make_daemon(foreground=None)
    daemon.handle_message({"type": "nav", "to": "next", "session": "x"})
    assert speaker.earcons == ["nav_edge"]


def test_streaming_content_does_not_move_the_cursor_but_flush_resets_it():
    # The streaming-nav bug fix: new paragraphs arriving while you navigate must
    # NOT yank the cursor to latest; only a new prompt (FLUSH) clears it.
    daemon, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "prev")
    anchored = daemon._nav_cursor.get("fg")
    assert anchored is not None
    # more content streams in -> cursor stays put
    daemon.handle_message({"type": "prose", "session": "fg",
                           "delta": "More streamed text.", "index": 9, "final": False})
    assert daemon._nav_cursor.get("fg") == anchored
    # a new prompt clears navigation
    daemon.handle_message({"type": "flush", "session": "fg"})
    assert "fg" not in daemon._nav_cursor


def test_nav_then_live_prose_continues_after_replay_no_interleave():
    # After navigating back, newly streamed prose enqueues AFTER the replayed
    # items (a contiguous catch-up) rather than jumping into the middle of the
    # replay. Seek-and-play makes the in-between items play seamlessly.
    daemon, *_ = make_daemon(foreground="fg")
    _seed(daemon)                                    # m0, m1, m2
    _drain_channel(daemon)                           # clear initial channel state
    _nav(daemon, "prev")                             # inserts m1, m2 at cursor
    daemon.handle_message({"type": "prose", "session": "fg",
                           "delta": "Live continues.\n\n", "index": 7, "final": False})
    texts = [s.text for s in _drain_channel(daemon)]
    assert texts[:2] == ["m1", "m2"]
    assert "Live continues." in texts
    assert texts.index("Live continues.") > texts.index("m2")   # after, not interleaved


def test_nav_with_empty_history_announces():
    daemon, *_ = make_daemon(foreground="fg")
    _nav(daemon, "prev")
    ch = daemon.router.channel("fg")
    assert any("Nothing to navigate" in it.text for it in ch.items)


def test_nav_steps_by_paragraph_within_one_message():
    daemon, *_ = make_daemon(foreground="fg")
    daemon.handle_message({
        "type": "prose", "session": "fg",
        "delta": "Para one sentence.\n\nPara two sentence.\n\nPara three sentence.",
        "index": 0, "final": True})
    _drain_channel(daemon)                           # clear the channel
    # the one message became three paragraph 'items'
    assert len(daemon.history.message_ids("fg")) == 3
    _nav(daemon, "prev")                             # latest(para3) -> para2 onward
    assert [s.text for s in _drain_channel(daemon)] == ["Para two sentence.", "Para three sentence."]
    _nav(daemon, "first")                            # -> para1 onward (whole message)
    assert [s.text for s in _drain_channel(daemon)] == [
        "Para one sentence.", "Para two sentence.", "Para three sentence."]
    _nav(daemon, "last")                             # -> para3 only
    assert [s.text for s in _drain_channel(daemon)] == ["Para three sentence."]
