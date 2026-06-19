from sonari.protocol import MsgType, PROTOCOL_VERSION
from sonari.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _msg(mtype, session, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype, "session": session}
    d.update(extra)
    return d


def _channel_items(daemon, session):
    """All items in the session's channel (at or after cursor)."""
    ch = daemon.router.channel(session)
    return list(ch.items[ch.cursor:])


def _channel_pop(daemon, session):
    """Pop and return the next item from the session's channel."""
    ch = daemon.router.channel(session)
    if ch.cursor >= len(ch.items):
        return None
    item = ch.items[ch.cursor]
    ch.cursor += 1
    return item


def test_choice_enqueues_when_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "fg", questions=[
        {"question": "Pick a color", "options": [{"label": "Red"}, {"label": "Blue"}]},
    ]))
    # A content message NEVER earcons; the alert is a separate EARCON message.
    assert speaker.earcons == []
    items = _channel_items(daemon, "fg")
    assert len(items) == 1
    item = items[0]
    assert item.kind == "choice"
    assert item.is_decision is True
    assert "Pick a color" in item.text
    assert "Option 1: Red." in item.text
    assert "Option 2: Blue." in item.text


def test_plan_enqueues_when_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PLAN, "fg", text="Step one then step two."))
    assert speaker.earcons == []
    items = _channel_items(daemon, "fg")
    assert len(items) == 1
    item = items[0]
    assert item.kind == "plan"
    assert item.is_decision is True
    assert "Step one then step two." in item.text


def test_permission_enqueues_when_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PERMISSION, "fg", action="run rm -rf"))
    assert speaker.earcons == []
    items = _channel_items(daemon, "fg")
    assert len(items) == 1
    item = items[0]
    assert item.kind == "permission"
    assert item.is_decision is True
    assert "run rm -rf" in item.text


def test_decision_lands_in_its_own_channel_regardless_of_foreground():
    # In the channel architecture, decisions always go into the session's channel;
    # the router's preemption logic determines when they are spoken.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.CHOICE, "other", questions=[{"question": "Q"}]))
    # Content messages never earcon (the EARCON message does).
    assert speaker.earcons == []
    # The decision is in the 'other' session's channel (not the 'fg' channel).
    other_items = _channel_items(daemon, "other")
    assert len(other_items) == 1
    assert other_items[0].kind == "choice"
    # The fg channel has nothing from this.
    fg_items = _channel_items(daemon, "fg")
    assert len(fg_items) == 0


def test_tool_announce_enqueues_only_when_verbosity_everything():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "fg", tool="Bash", summary="run tests"))
    items = _channel_items(daemon, "fg")
    assert len(items) == 1
    item = items[0]
    assert item.kind == "tool_announce"
    assert item.is_decision is False
    assert "run tests" in item.text


def test_tool_announce_dropped_when_verbosity_medium():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "fg", tool="Bash", summary="run tests"))
    assert len(_channel_items(daemon, "fg")) == 0


def test_tool_announce_dropped_when_verbosity_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "fg", tool="Bash", summary="run tests"))
    assert len(_channel_items(daemon, "fg")) == 0


def test_tool_announce_dropped_when_not_foreground():
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    daemon.handle_message(_msg(MsgType.TOOL, "other", tool="Bash", summary="run tests"))
    assert len(_channel_items(daemon, "other")) == 0


def test_decision_enqueued_at_everything():
    for mtype, kwargs, kind in [
        (MsgType.CHOICE, {"questions": [{"question": "Q?"}]}, "choice"),
        (MsgType.PLAN, {"text": "Do X."}, "plan"),
        (MsgType.PERMISSION, {"action": "rm -rf"}, "permission"),
    ]:
        daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
        daemon.handle_message(_msg(mtype, "fg", **kwargs))
        items = _channel_items(daemon, "fg")
        assert len(items) == 1, f"{kind} not enqueued at everything"
        assert items[0].kind == kind


def test_decision_enqueued_at_medium():
    for mtype, kwargs, kind in [
        (MsgType.CHOICE, {"questions": [{"question": "Q?"}]}, "choice"),
        (MsgType.PLAN, {"text": "Do X."}, "plan"),
        (MsgType.PERMISSION, {"action": "rm -rf"}, "permission"),
    ]:
        daemon, queue, speaker, sessions, config = make_daemon(verbosity="medium", foreground="fg")
        daemon.handle_message(_msg(mtype, "fg", **kwargs))
        items = _channel_items(daemon, "fg")
        assert len(items) == 1, f"{kind} not enqueued at medium"
        assert items[0].kind == kind


def test_decision_enqueued_at_quiet():
    for mtype, kwargs, kind in [
        (MsgType.CHOICE, {"questions": [{"question": "Q?"}]}, "choice"),
        (MsgType.PLAN, {"text": "Do X."}, "plan"),
        (MsgType.PERMISSION, {"action": "rm -rf"}, "permission"),
    ]:
        daemon, queue, speaker, sessions, config = make_daemon(verbosity="quiet", foreground="fg")
        daemon.handle_message(_msg(mtype, "fg", **kwargs))
        items = _channel_items(daemon, "fg")
        assert len(items) == 1, f"{kind} not enqueued at quiet"
        assert items[0].kind == kind


def test_jump_decision_advances_channel_cursor_to_decision():
    """M6: JUMP_DECISION advances the channel cursor past non-decision items
    to the next decision, drops their heard-markers, and marks the current
    item heard so a later CATCH_UP doesn't replay them out of order."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # A current item being spoken, with a heard-marker entry.
    cur_entry = daemon.history.record("fg", "prose", "current")
    cur = SpeechItem(id=99, session="fg", kind="prose", text="current", is_decision=False)
    daemon._current_item = cur
    daemon._pending_heard[cur.id] = cur_entry
    # Two queued prose items (with heard-markers) ahead of a decision, in the channel.
    e1 = daemon.history.record("fg", "prose", "p1")
    e2 = daemon.history.record("fg", "prose", "p2")
    daemon._enqueue("fg", "prose", "p1", False, entry=e1)
    daemon._enqueue("fg", "prose", "p2", False, entry=e2)
    daemon._enqueue("fg", "choice", "decide", True)
    ch = daemon.router.channel("fg")
    prose_ids = [it.id for it in ch.items if not it.is_decision]
    assert all(pid in daemon._pending_heard for pid in prose_ids)

    daemon.handle_message({"type": "jump_decision", "session": "fg"})

    assert speaker.cancels == 1
    assert cur_entry.heard is True                 # cancelled current marked heard
    # The dropped prose items' pending-heard entries are gone (no leak).
    assert all(pid not in daemon._pending_heard for pid in prose_ids)
    # The channel cursor now points at the decision item.
    decision_item = ch.items[ch.cursor]
    assert decision_item.text == "decide"
    assert decision_item.is_decision is True


def test_bare_earcon_message_plays_kind():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.EARCON, "fg", kind="turn_done"))
    assert speaker.earcons == ["turn_done"]
    assert len(_channel_items(daemon, "fg")) == 0
