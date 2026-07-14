from sonara.protocol import MsgType, PROTOCOL_VERSION
from sonara.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


def _seed(queue, daemon, session, n, decision_at=None):
    for i in range(n):
        is_dec = decision_at is not None and i == decision_at
        queue.enqueue(SpeechItem(
            id=daemon._alloc_id(),
            session=session,
            kind="plan" if is_dec else "prose",
            text="item {0}".format(i),
            is_decision=is_dec,
        ))


def test_flush_drops_session_items_without_cancelling_other_speech():
    # Flush now cancels only when the current utterance belongs to the flushed
    # session. There is no current utterance in this unit test, so no cancel.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _seed(queue, daemon, "fg", 2)
    _seed(queue, daemon, "other", 1)
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    assert speaker.cancels == 0
    # only the 'other' session item remains
    assert len(queue) == 1
    assert queue.pop_next().session == "other"


def test_stop_clears_all_and_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _seed(queue, daemon, "fg", 3)
    daemon.handle_message(_msg(MsgType.STOP, "fg"))
    assert len(queue) == 0
    assert speaker.cancels == 1


def test_skip_only_cancels_current():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _seed(queue, daemon, "fg", 3)
    daemon.handle_message(_msg(MsgType.SKIP, "fg"))
    assert speaker.cancels == 1
    # queue untouched by skip
    assert len(queue) == 3


def test_jump_decision_drops_to_first_decision_and_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # items 0,1 prose; item 2 is a decision
    _seed(queue, daemon, "fg", 4, decision_at=2)
    daemon.handle_message(_msg(MsgType.JUMP_DECISION, "fg"))
    assert speaker.cancels == 1
    nxt = queue.pop_next()
    assert nxt.is_decision is True
    assert nxt.text == "item 2"


def test_catch_up_no_longer_discards_the_backlog():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PROSE, "fg", delta="Keep me. ",
                               index=0, final=True))
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = [queue.pop_next().text for _ in range(len(queue))]
    assert "Keep me." in texts


def test_set_foreground_sets_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon.handle_message(_msg(MsgType.SET_FOREGROUND, "s9"))
    assert sessions.foreground() == "s9"


def test_session_start_sets_foreground_and_registers():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon._setup_health = lambda v: ("ok", None)  # keep focus on fg/register
    daemon.handle_message(_msg(MsgType.SESSION_START, "s9"))
    assert sessions.foreground() == "s9"
    assert sessions.is_foreground("s9") is True


def test_session_end_unregisters():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="s9")
    daemon.handle_message(_msg(MsgType.SESSION_END, "s9"))
    assert sessions.foreground() is None


# ---------------------------------------------------------------------------
# REPEAT tests (history-based -- see Phase 2.1)
# ---------------------------------------------------------------------------

def _prose(daemon, session, text, index=0, final=True):
    daemon.handle_message(_msg(MsgType.PROSE, session, delta=text,
                               index=index, final=final))


def _drain_one(daemon, queue, speaker):
    item = queue.pop_next()
    assert item is not None
    completed = speaker.speak(item.text)
    daemon.note_spoken(item, completed)
    return item


def test_repeat_noop_when_nothing_spoken_yet():
    """REPEAT before any speech says "Nothing to repeat." when foreground exists."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.REPEAT, "fg"))
    item = queue.pop_next()
    assert item.text == "Nothing to repeat."


def test_repeat_reenqueues_last_spoken_text():
    """After prose is spoken, REPEAT re-enqueues the whole last message."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Hello world. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REPEAT, "fg"))
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.text == "Hello world."
    assert item.kind == "prose"
    assert item.session == "fg"
    assert item.is_decision is False


def test_repeat_noop_when_no_foreground_session():
    """REPEAT with no foreground session must not enqueue anything."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon.handle_message(_msg(MsgType.REPEAT))
    assert len(queue) == 0


def test_repeat_drives_speak_path():
    """Integration: enqueue prose, drain it, REPEAT, then drain again through
    the speak path and assert the text is spoken a second time."""
    import threading, time

    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Repeat me please. ")

    # Kick the speak loop.
    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    try:
        deadline = time.time() + 2.0
        while time.time() < deadline and not speaker.spoken:
            time.sleep(0.01)
        assert "Repeat me please." in speaker.spoken

        # Issue REPEAT after the first round has been spoken.
        daemon.handle_message(_msg(MsgType.REPEAT, "fg"))

        deadline = time.time() + 2.0
        initial_count = len(speaker.spoken)
        while time.time() < deadline and len(speaker.spoken) <= initial_count:
            time.sleep(0.01)

        assert speaker.spoken.count("Repeat me please.") >= 2
    finally:
        daemon.stop()
        t.join(timeout=2.0)
