from tests.daemon_helpers import make_daemon
from sonari.protocol import MsgType, PROTOCOL_VERSION


def _prose(s, delta, idx, final):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s,
            "delta": delta, "index": idx, "final": final}


def test_prose_lands_in_the_sessions_channel():
    daemon, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Hello there. ", 0, True))
    ch = daemon.router.channel("A")
    assert [i.text for i in ch.items] == ["Hello there."]
    assert ch.turn_done is True


def test_new_prompt_wipes_only_its_own_channel():
    daemon, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "A first. ", 0, True))
    daemon.handle_message(_prose("B", "B first. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "A"})
    assert daemon.router.channel("A").items == []          # A wiped
    assert [i.text for i in daemon.router.channel("B").items] == ["B first."]  # B intact
