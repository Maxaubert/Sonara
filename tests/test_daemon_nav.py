"""Up (nav 'first'): restart the latest turn from the top.

One message, always the last: Sonara reads the latest turn and Up replays it
from its first message, cutting current speech and chiming 'nav'. With nothing
recorded it chimes 'nav_edge'. There is no stepping between paragraphs or
older turns: any other nav target is a silent no-op."""
from sonara.protocol import MsgType, PROTOCOL_VERSION
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


def _start_reading(daemon, session, msg_id):
    """Simulate the speak loop currently reading a given message."""
    from sonara.queue import SpeechItem
    entry = daemon.history.entries_for_message(session, msg_id)[0]
    item = SpeechItem(id=9000 + msg_id, session=session, kind="prose",
                      text=entry.text, is_decision=False)
    daemon._pending_heard[item.id] = entry
    daemon._current_item = item


def test_repeat_reads_last_message_via_channel():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Hello. ", 0, True))
    daemon._playback.run_once()                     # reads "Hello."
    speaker.spoken.clear()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.REPEAT})
    daemon._playback.run_once()
    assert speaker.spoken == ["Hello."]


def test_up_replays_the_whole_turn_from_the_top():
    daemon, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "first")
    assert [s.text for s in _drain_channel(daemon)] == ["m0a", "m0b", "m1", "m2"]


def test_up_fires_nav_chime_and_cuts_current_speech():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _nav(daemon, "first")
    assert speaker.earcons == ["nav"]
    assert speaker.cancels == 1


def test_up_while_reading_a_later_message_restarts_from_the_first():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    _start_reading(daemon, "fg", 1)                  # currently hearing m1
    _nav(daemon, "first")
    assert speaker.earcons[-1] == "nav"
    assert [s.text for s in _drain_channel(daemon)] == ["m0a", "m0b", "m1", "m2"]


def test_repeated_up_always_chimes_nav_and_replays():
    # Up is a restart, not a step: every press re-reads the turn from the top
    # and chimes "nav" (#128).
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    for _ in range(3):
        _nav(daemon, "first")
    assert speaker.earcons == ["nav", "nav", "nav"]
    assert [it.text for it in _drain_channel(daemon)] == ["m0a", "m0b", "m1", "m2"]


def test_up_on_single_message_turn_chimes_nav():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.history.record("fg", "prose", "only")
    _nav(daemon, "first")
    assert speaker.earcons == ["nav"]
    assert [it.text for it in _drain_channel(daemon)] == ["only"]


def test_up_with_no_history_fires_nav_edge_and_announces():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")   # nothing recorded
    _nav(daemon, "first")
    assert speaker.earcons == ["nav_edge"]
    from sonara.router import CONTROL                # a control cue (F6, #137)
    ch = daemon.router.channel(CONTROL)
    assert any("Nothing to navigate" in it.text for it in ch.items)
    assert daemon.router.channel("fg").items == []


def test_up_with_no_foreground_fires_nav_edge():
    daemon, queue, speaker, *_ = make_daemon(foreground=None)
    daemon.handle_message({"type": "nav", "to": "first", "session": "x"})
    assert speaker.earcons == ["nav_edge"]


def test_nav_without_a_target_means_up():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    daemon.handle_message({"type": "nav", "session": "fg"})
    assert speaker.earcons == ["nav"]
    assert [s.text for s in _drain_channel(daemon)] == ["m0a", "m0b", "m1", "m2"]


def test_removed_nav_targets_are_silent_noops():
    # D1: prev/next/last stepping was removed. A stale client sending them gets
    # nothing: no chime, no cut, nothing replayed.
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    _seed(daemon)
    for to in ("prev", "next", "last", "bogus"):
        _nav(daemon, to)
    assert speaker.earcons == []
    assert speaker.cancels == 0
    assert daemon.router.channel("fg").pending() == 0


def test_daemon_has_no_nav_cursor_state():
    # The per-session paragraph cursor only served prev/next; it is gone.
    daemon, *_ = make_daemon(foreground="fg")
    assert not hasattr(daemon, "_nav_cursor")
    assert not hasattr(daemon, "_reading_msg_id")


def test_up_then_live_prose_continues_after_replay_no_interleave():
    # After Up, newly streamed prose enqueues AFTER the replayed items rather
    # than jumping into the middle of the replay.
    daemon, *_ = make_daemon(foreground="fg")
    _seed(daemon)                                    # m0, m1, m2
    _drain_channel(daemon)                           # clear initial channel state
    _nav(daemon, "first")
    daemon.handle_message({"type": "prose", "session": "fg",
                           "delta": "Live continues.\n\n", "index": 7, "final": False})
    texts = [s.text for s in _drain_channel(daemon)]
    assert texts[:4] == ["m0a", "m0b", "m1", "m2"]
    assert "Live continues." in texts
    assert texts.index("Live continues.") > texts.index("m2")   # after, not interleaved


def test_engaged_session_is_the_active_reader_not_the_foreground():
    # After a session change the active reader differs from the foreground. The
    # session the user is ENGAGED with (and restarts with Up) is what they HEAR.
    daemon, *_ = make_daemon(foreground="B")
    daemon.router.active = "A"
    assert daemon._engaged_session() == "A"           # active reader wins
    daemon.router.active = None
    daemon.router._last_active = "A"
    assert daemon._engaged_session() == "A"           # last reader persists across idle
    daemon.router._last_active = None
    assert daemon._engaged_session() == "B"           # falls back to foreground


def test_up_restarts_the_engaged_reader_not_the_foreground():
    # Content was read for session A (the active reader) while the foreground is
    # B (no turn of its own). Up must restart A's turn, not find nothing in B.
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="B")
    h = daemon.history
    h.record("A", "prose", "a0"); h.end_message("A")
    h.record("A", "prose", "a1")                      # A has a 2-message turn
    daemon.router.active = "A"                        # A is what the user hears
    daemon.handle_message({"type": "nav", "to": "first", "session": "B"})
    assert speaker.earcons[-1] == "nav"
    ch = daemon.router.channel("A")
    assert [it.text for it in ch.items[ch.cursor:]] == ["a0", "a1"]   # replayed A


def test_up_replays_every_paragraph_of_a_message():
    daemon, *_ = make_daemon(foreground="fg")
    daemon.handle_message({
        "type": "prose", "session": "fg",
        "delta": "Para one sentence.\n\nPara two sentence.\n\nPara three sentence.",
        "index": 0, "final": True})
    _drain_channel(daemon)                           # clear the channel
    _nav(daemon, "first")
    assert [s.text for s in _drain_channel(daemon)] == [
        "Para one sentence.", "Para two sentence.", "Para three sentence."]
