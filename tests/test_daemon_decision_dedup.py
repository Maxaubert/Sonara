"""AskUserQuestion fires BOTH a PreToolUse (choice earcon + question) and, ~5-6s
later, a permission-prompt Notification (permission earcon + "Claude needs your
permission"). That permission is redundant with the question already announced,
so while a question is unanswered the daemon suppresses the permission (its earcon
AND its spoken text). Genuine permissions (no pending question) are untouched."""
from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _choice(daemon, session="fg"):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE, "session": session,
                           "questions": [{"question": "Pick?", "options": ["a", "b"]}]})


def _permission(daemon, session="fg"):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PERMISSION, "session": session,
                           "action": "", "message": "Claude needs your permission"})


def _perm_earcon(daemon):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "permission"})


def _prose(daemon, session="fg", idx=5):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": session,
                           "delta": "Continuing. ", "index": idx, "final": True})


def _pending_kinds(daemon, session="fg"):
    ch = daemon.router.channel(session)
    return [it.kind for it in ch.items[ch.cursor:]]


def test_permission_content_suppressed_while_question_unanswered():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon)
    _permission(daemon)
    assert "permission" not in _pending_kinds(daemon)   # redundant permission dropped


def test_permission_earcon_suppressed_while_question_unanswered():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon)
    speaker.earcons.clear()
    _perm_earcon(daemon)
    assert "permission" not in speaker.earcons          # no redundant second chime


def test_genuine_permission_content_still_spoken():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _permission(daemon)                                  # no pending question
    assert "permission" in _pending_kinds(daemon)


def test_genuine_permission_earcon_still_fires():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    speaker.earcons.clear()
    _perm_earcon(daemon)
    assert "permission" in speaker.earcons


def test_pre_question_prose_does_not_reenable_permission():
    # THE bug (confirmed via message-sequence capture): the "Here's the question:"
    # prose streams in AFTER the choice, so clearing the guard on prose let the
    # permission (6s later) slip through. Prose must NOT reenable it.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon)
    _prose(daemon)                                        # late pre-question prose
    _permission(daemon)
    assert "permission" not in _pending_kinds(daemon)     # still suppressed


def test_permission_reenabled_after_it_suppressed_one():
    # The guard is CONSUMED by the permission it suppresses: a SECOND permission
    # (e.g. a genuine one later in the turn) is spoken normally.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon)
    _permission(daemon)                                   # suppressed + consumes guard
    assert "permission" not in _pending_kinds(daemon)
    _permission(daemon)                                   # second one is genuine
    assert "permission" in _pending_kinds(daemon)


def test_permission_reenabled_after_new_prompt():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    _permission(daemon)
    assert "permission" in _pending_kinds(daemon)
