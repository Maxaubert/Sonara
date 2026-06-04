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
