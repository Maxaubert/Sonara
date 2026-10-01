from sonara.protocol import MsgType, PROTOCOL_VERSION
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


def test_tool_announce_lands_in_background_channel_not_spoken_until_active():
    # A tool announcement for a background session must land in THAT session's own
    # channel regardless of foreground. The Router decides when it is read: it serves
    # a session's channel only when that session is the active reader, so the bg tool
    # text must NOT be spoken while a different session holds the voice.
    from sonara.protocol import PROTOCOL_VERSION
    daemon, queue, speaker, sessions, config = make_daemon(verbosity="everything", foreground="fg")
    # Give "fg" some prose so it stays the active reader.
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE,
                           "session": "fg", "delta": "Foreground prose. ", "index": 0, "final": True})
    daemon.handle_message(_msg(MsgType.TOOL, "other", tool="Bash", summary="run tests"))
    # The item lands in "other"'s own channel.
    items = _channel_items(daemon, "other")
    assert len(items) == 1
    assert items[0].kind == "tool_announce"
    assert "run tests" in items[0].text
    # Drive the speak loop: "fg" is the active reader, so only fg's prose is spoken.
    # The bg tool text must NOT be spoken while fg holds the voice.
    daemon._speak_loop_once()
    assert any("Foreground prose" in t for t in speaker.spoken)
    assert "run tests" not in speaker.spoken


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
