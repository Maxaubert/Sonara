from sonari.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(mtype, session, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype, "session": session}
    d.update(extra)
    return d


def test_choice_enqueues_when_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Pick a color", "options": [{"label": "Red"}, {"label": "Blue"}]},
    ]))
    # A content message NEVER earcons; the alert is a separate EARCON message.
    assert speaker.earcons == []
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "choice"
    assert item.is_decision is True
    assert "Pick a color" in item.text
    assert "Option 1: Red." in item.text
    assert "Option 2: Blue." in item.text


def test_plan_enqueues_when_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Step one then step two."))
    assert speaker.earcons == []
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "plan"
    assert item.is_decision is True
    assert "Step one then step two." in item.text


def test_permission_enqueues_when_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PERMISSION, "fg", action="run rm -rf"))
    assert speaker.earcons == []
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "permission"
    assert item.is_decision is True
    assert "run rm -rf" in item.text


def test_decision_content_not_enqueued_when_not_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "other", questions=[{"question": "Q"}]))
    # Content messages never earcon (the EARCON message does), and a
    # non-foreground decision's spoken text is not enqueued.
    assert speaker.earcons == []
    assert len(queue) == 0


def test_tool_announce_enqueues_only_when_verbosity_everything():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "fg", tool="Bash", summary="run tests"))
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.kind == "tool_announce"
    assert item.is_decision is False
    assert "run tests" in item.text


def test_tool_announce_dropped_when_verbosity_medium():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "fg", tool="Bash", summary="run tests"))
    assert len(queue) == 0


def test_tool_announce_dropped_when_verbosity_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "fg", tool="Bash", summary="run tests"))
    assert len(queue) == 0


def test_tool_announce_dropped_when_not_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "other", tool="Bash", summary="run tests"))
    assert len(queue) == 0


def test_decision_enqueued_at_everything():
    for mtype, kwargs, kind in [
        (MsgType.CHOICE, {"questions": [{"question": "Q?"}]}, "choice"),
        (MsgType.PLAN, {"text": "Do X."}, "plan"),
        (MsgType.PERMISSION, {"action": "rm -rf"}, "permission"),
    ]:
        daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
        daemon.handle_message(_msg(mtype, "fg", **kwargs))
        assert len(queue) == 1, f"{kind} not enqueued at everything"
        assert queue.pop_next().kind == kind


def test_decision_enqueued_at_medium():
    for mtype, kwargs, kind in [
        (MsgType.CHOICE, {"questions": [{"question": "Q?"}]}, "choice"),
        (MsgType.PLAN, {"text": "Do X."}, "plan"),
        (MsgType.PERMISSION, {"action": "rm -rf"}, "permission"),
    ]:
        daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
        daemon.handle_message(_msg(mtype, "fg", **kwargs))
        assert len(queue) == 1, f"{kind} not enqueued at medium"
        assert queue.pop_next().kind == kind


def test_decision_enqueued_at_quiet():
    for mtype, kwargs, kind in [
        (MsgType.CHOICE, {"questions": [{"question": "Q?"}]}, "choice"),
        (MsgType.PLAN, {"text": "Do X."}, "plan"),
        (MsgType.PERMISSION, {"action": "rm -rf"}, "permission"),
    ]:
        daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
        daemon.handle_message(_msg(mtype, "fg", **kwargs))
        assert len(queue) == 1, f"{kind} not enqueued at quiet"
        assert queue.pop_next().kind == kind


def test_bare_earcon_message_plays_kind():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.EARCON, "fg", kind="turn_done"))
    assert speaker.earcons == ["turn_done"]
    assert len(queue) == 0
