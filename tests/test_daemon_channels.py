from tests.daemon_helpers import make_daemon
from sonara.protocol import MsgType, PROTOCOL_VERSION


def _prose(s, delta, idx, final):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s,
            "delta": delta, "index": idx, "final": final}


def test_prose_lands_in_the_sessions_channel():
    daemon, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Hello there. ", 0, True))
    ch = daemon.router.channel("A")
    assert [i.text for i in ch.items] == ["Hello there."]
    # PROSE final=True closes the text block but does NOT set turn_done —
    # Claude Code marks each streamed text block final, and the minqueue design
    # batches across blocks until the turn_done earcon or FLUSH arrives.
    assert ch.turn_done is False


def test_new_prompt_wipes_only_its_own_channel():
    daemon, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "A first. ", 0, True))
    daemon.handle_message(_prose("B", "B first. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "A"})
    assert daemon.router.channel("A").items == []          # A wiped
    assert [i.text for i in daemon.router.channel("B").items] == ["B first."]  # B intact


def test_speak_loop_reads_active_channel_then_idles():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "One. Two. ", 0, True))
    daemon._speak_loop_once()
    daemon._speak_loop_once()
    assert speaker.spoken == ["One.", "Two."]
    daemon._speak_loop_once()                 # nothing left -> idle, no error
    assert speaker.spoken == ["One.", "Two."]


def test_flush_drops_pending_heard_for_wiped_channel():
    daemon, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Held. ", 0, False))   # un-spoken item in channel
    assert len(daemon._pending_heard) >= 1
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "A"})
    assert daemon._pending_heard == {}                       # no leak
