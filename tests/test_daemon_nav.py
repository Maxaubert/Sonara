"""Message-cursor navigation (nav next/prev/first/last) over the current turn.
Each move cuts current speech, clears the queue, and replays the target message;
next/prev clamp at the ends (no wrap); new content snaps the cursor to latest."""
from tests.daemon_helpers import make_daemon


def _drain(queue):
    items = []
    while True:
        it = queue.pop_next()
        if it is None:
            break
        items.append(it)
    return items


def _seed(daemon):
    # Current turn = 3 messages: m0 (two sentences), m1, m2 (latest).
    h = daemon.history
    h.record("fg", "prose", "m0a"); h.record("fg", "prose", "m0b"); h.end_message("fg")
    h.record("fg", "prose", "m1"); h.end_message("fg")
    h.record("fg", "prose", "m2")


def _nav(daemon, to):
    daemon.handle_message({"type": "nav", "to": to, "session": "fg"})


def test_prev_steps_back_one_message_then_plays_forward():
    # Seek-and-play: stepping back lands on the previous item AND reads every
    # later one, so playback continues instead of stopping after one item.
    daemon, queue, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "prev")
    assert [s.text for s in _drain(queue)] == ["m1", "m2"]
    _nav(daemon, "prev")
    assert [s.text for s in _drain(queue)] == ["m0a", "m0b", "m1", "m2"]


def test_prev_clamps_at_first():
    daemon, queue, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    for _ in range(5):
        _nav(daemon, "prev")
    assert [s.text for s in _drain(queue)] == ["m0a", "m0b", "m1", "m2"]
    assert daemon._nav_cursor["fg"] == 0


def test_next_clamps_at_last_no_wrap():
    daemon, queue, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "first"); _drain(queue)                       # cursor at m0
    _nav(daemon, "next"); assert [s.text for s in _drain(queue)] == ["m1", "m2"]
    _nav(daemon, "next"); assert [s.text for s in _drain(queue)] == ["m2"]
    _nav(daemon, "next")                                       # at last -> re-read m2
    assert [s.text for s in _drain(queue)] == ["m2"]           # never wrapped to 0
    # reaching the latest clears the cursor: "following live" again, not pinned
    assert daemon._nav_cursor.get("fg") is None


def test_first_and_last_jump():
    daemon, queue, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "first")
    assert [s.text for s in _drain(queue)] == ["m0a", "m0b", "m1", "m2"]   # whole turn
    _nav(daemon, "last")
    assert [s.text for s in _drain(queue)] == ["m2"]


def test_streaming_content_does_not_move_the_cursor_but_flush_resets_it():
    # The streaming-nav bug fix: new paragraphs arriving while you navigate must
    # NOT yank the cursor to latest; only a new prompt (FLUSH) clears it.
    daemon, queue, *_ = make_daemon(foreground="fg")
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
    # #4: after navigating back, newly streamed prose enqueues AFTER the replayed
    # items (a contiguous catch-up) rather than jumping into the middle of the
    # replay. Seek-and-play makes the in-between items play, so there is no jump.
    daemon, queue, *_ = make_daemon(foreground="fg")
    _seed(daemon)                                   # m0, m1, m2
    _drain(queue)
    _nav(daemon, "prev")                            # queues m1, m2 (seek-and-play)
    daemon.handle_message({"type": "prose", "session": "fg",
                           "delta": "Live continues.\n\n", "index": 7, "final": False})
    texts = [s.text for s in _drain(queue)]
    assert texts[:2] == ["m1", "m2"]
    assert "Live continues." in texts
    assert texts.index("Live continues.") > texts.index("m2")   # after, not interleaved


def test_nav_makes_foreground_session_the_voice_owner():
    """L3: navigating is an active foreground action; it must claim the voice so
    that prose streaming in after the replay is spoken, not captured, even if a
    background session currently owns the voice."""
    daemon, queue, *_ = make_daemon(foreground="fg")
    daemon._voice_owner = "bg"                 # a background session holds the voice
    daemon._captured_msg.add("fg")             # and fg's message was being captured
    _seed(daemon)
    _nav(daemon, "prev")
    assert daemon._voice_owner == "fg"         # nav reclaimed the voice for fg
    assert "fg" not in daemon._captured_msg
    # live prose for fg now enqueues (spoken), not captured
    _drain(queue)
    daemon.handle_message({"type": "prose", "session": "fg",
                           "delta": "Live after nav. ", "index": 9, "final": False})
    assert [s.text for s in _drain(queue)] == ["Live after nav."]


def test_nav_does_not_steal_voice_from_a_streaming_session():
    """L3 + review: nav claims a free/stale/own voice, but must NOT seize it from a
    different session still streaming a reply (owner in _open_msg) — that would
    strand the streamer mid-sentence, the very thing H1 prevents. The replay items
    are still enqueued regardless; only voice ownership is left untouched."""
    daemon, queue, *_ = make_daemon(foreground="fg")
    daemon._voice_owner = "a"            # a background session is mid-stream...
    daemon._open_msg.add("a")            # ...its message is open
    _seed(daemon)                         # fg has history to navigate
    _nav(daemon, "prev")
    assert daemon._voice_owner == "a"    # not stolen from the streamer
    assert len(queue) > 0                # fg's replay items still enqueued


def test_nav_with_empty_history_announces():
    daemon, queue, *_ = make_daemon(foreground="fg")
    _nav(daemon, "prev")
    assert any("Nothing to navigate" in s.text for s in _drain(queue))


def test_nav_steps_by_paragraph_within_one_message():
    daemon, queue, *_ = make_daemon(foreground="fg")
    daemon.handle_message({
        "type": "prose", "session": "fg",
        "delta": "Para one sentence.\n\nPara two sentence.\n\nPara three sentence.",
        "index": 0, "final": True})
    _drain(queue)                                   # clear the spoken queue
    # the one message became three paragraph 'items'
    assert len(daemon.history.message_ids("fg")) == 3
    _nav(daemon, "prev")                            # latest(para3) -> para2 onward
    assert [s.text for s in _drain(queue)] == ["Para two sentence.", "Para three sentence."]
    _nav(daemon, "first")                           # -> para1 onward (whole message)
    assert [s.text for s in _drain(queue)] == [
        "Para one sentence.", "Para two sentence.", "Para three sentence."]
    _nav(daemon, "last")                            # -> para3 only
    assert [s.text for s in _drain(queue)] == ["Para three sentence."]
