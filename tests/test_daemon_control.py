from echo.protocol import MsgType, PROTOCOL_VERSION
from echo.queue import SpeechItem
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


def test_flush_drops_session_items_and_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _seed(queue, daemon, "fg", 2)
    _seed(queue, daemon, "other", 1)
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    assert speaker.cancels == 1
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


def test_catch_up_clears_and_cancels():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _seed(queue, daemon, "fg", 3)
    daemon.handle_message(_msg(MsgType.CATCH_UP, "fg"))
    assert len(queue) == 0
    assert speaker.cancels == 1


def test_set_foreground_sets_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon.handle_message(_msg(MsgType.SET_FOREGROUND, "s9"))
    assert sessions.foreground() == "s9"


def test_session_start_sets_foreground_and_registers():
    daemon, queue, speaker, sessions, config = make_daemon(foreground=None)
    daemon.handle_message(_msg(MsgType.SESSION_START, "s9"))
    assert sessions.foreground() == "s9"
    assert sessions.is_foreground("s9") is True


def test_session_end_unregisters():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="s9")
    daemon.handle_message(_msg(MsgType.SESSION_END, "s9"))
    assert sessions.foreground() is None


# ---------------------------------------------------------------------------
# REPEAT tests
# ---------------------------------------------------------------------------

def test_repeat_noop_when_nothing_spoken_yet():
    """REPEAT before any speech must not enqueue anything and must not crash."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon._last_spoken is None
    daemon.handle_message(_msg(MsgType.REPEAT, "fg"))
    assert len(queue) == 0


def test_repeat_reenqueues_last_spoken_text():
    """After _last_spoken is set, REPEAT re-enqueues that text as a prose item
    for the foreground session."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # Simulate the speak loop having spoken an item.
    daemon._last_spoken = "Hello world."
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
    daemon._last_spoken = "Something said earlier."
    daemon.handle_message(_msg(MsgType.REPEAT))
    assert len(queue) == 0


def test_repeat_drives_speak_path():
    """Integration: set _last_spoken then REPEAT, then drain queue through
    the speak path and assert the text is spoken again."""
    import threading, time

    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._last_spoken = "Repeat me please."

    # Kick the speak loop.
    t = threading.Thread(target=daemon._speak_loop, daemon=True)
    t.start()
    try:
        # Issue REPEAT while the loop is running.
        daemon.handle_message(_msg(MsgType.REPEAT, "fg"))

        deadline = time.time() + 2.0
        while time.time() < deadline and not speaker.spoken:
            time.sleep(0.01)

        assert "Repeat me please." in speaker.spoken
        # After speaking, _last_spoken should be updated to that text.
        # Give the loop a moment to persist _last_spoken.
        time.sleep(0.05)
        assert daemon._last_spoken == "Repeat me please."
    finally:
        daemon.stop()
        t.join(timeout=2.0)
