"""#174: hook events are separate processes on separate connections, so text
from the OLD turn (MessageDisplay, Stop) can reach the daemon after the new
prompt's FLUSH. Every hook message carries "t", its process start time; the
daemon ignores old-turn prose and turn_done stamped before the session's last
FLUSH, so the stale tail is never read as the new answer."""
from __future__ import annotations

from daemon_helpers import make_daemon
from sonara.protocol import PROTOCOL_VERSION, MsgType


def P(s, d, i, t, final=False):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s,
            "delta": d, "index": i, "final": final, "t": t}


def FL(s, t):
    return {"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": s, "t": t}


def TD(s, t):
    return {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "kind": "turn_done",
            "session": s, "t": t}


def _pending(daemon, session="fg"):
    return [it.text for it in daemon.router.channel(session).pending_items()]


def test_late_old_prose_after_flush_is_not_read():
    d, q, *_ = make_daemon(verbosity="medium", foreground="fg")
    d.handle_message(P("fg", "Old one. Old two. ", 0, t=100.0))
    d.handle_message(FL("fg", t=105.0))                               # prompt overtakes
    d.handle_message(P("fg", "Old three. Old four.", 1, t=104.0, final=True))
    d.handle_message(TD("fg", t=104.5))                               # late old Stop
    assert _pending(d) == []
    assert q.pop_next() is None


def test_late_old_block_at_index_zero_does_not_lead_the_new_turn():
    # Claude restarts block indexes at 0 after a tool call, so the old turn's
    # last block usually starts at 0 too: numbering cannot tell turns apart.
    d, q, *_ = make_daemon(verbosity="medium", foreground="fg")
    d.config["minqueue"] = 1
    d.handle_message(P("fg", "Old one. ", 0, t=100.0))
    d.handle_message(TD("fg", t=101.0))
    d.handle_message(FL("fg", t=105.0))
    d.handle_message(P("fg", "Late tail. Late two.", 0, t=103.0, final=True))
    d.handle_message(P("fg", "New A. New B. ", 0, t=106.0))
    assert _pending(d) == ["New A.", "New B."]


def test_stale_index_does_not_swallow_the_new_turns_text():
    d, q, *_ = make_daemon(verbosity="medium", foreground="fg")
    d.handle_message(FL("fg", t=105.0))
    d.handle_message(P("fg", "Stale tail. ", 2, t=104.0))
    for i, text in enumerate(["New A. ", "New B. ", "New C. ", "New D. "]):
        d.handle_message(P("fg", text, i, t=106.0 + i, final=(i == 3)))
    assert _pending(d) == ["New A.", "New B.", "New C.", "New D."]


def test_late_old_turn_done_plays_no_chime_and_does_not_release_the_new_turn():
    d, q, speaker, *_ = make_daemon(verbosity="medium", foreground="fg")
    d.config["minqueue"] = 5
    d.handle_message(FL("fg", t=105.0))
    d.handle_message(P("fg", "New A. ", 0, t=106.0))
    d.handle_message(TD("fg", t=104.0))                               # old turn's Stop
    assert "turn_done" not in speaker.earcons
    assert d.router.channel("fg").turn_done is False


def test_unstamped_messages_behave_as_before():
    # An older hook (no "t") must keep working exactly as it did.
    d, q, *_ = make_daemon(verbosity="medium", foreground="fg")
    d.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    d.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": "fg",
                      "delta": "Hello there.", "index": 0, "final": True})
    assert _pending(d) == ["Hello there."]


def test_a_flush_only_guards_its_own_session():
    d, q, *_ = make_daemon(verbosity="medium", foreground="fg")
    d.handle_message(FL("fg", t=105.0))
    d.handle_message(P("other", "Other reply.", 0, t=104.0, final=True))
    assert _pending(d, "other") == ["Other reply."]
