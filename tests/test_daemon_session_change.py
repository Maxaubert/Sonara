from tests.daemon_helpers import make_daemon
from sonara.protocol import MsgType, PROTOCOL_VERSION


def _prose(s, d, i, f):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s, "delta": d, "index": i, "final": f}


def _spoken(daemon, speaker, n=12):
    for _ in range(n):
        daemon._speak_loop_once()
    return speaker.spoken


def test_next_session_switches_to_other_unread_and_announces():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "A one. ", 0, True))
    daemon.handle_message(_prose("B", "B one. ", 0, True))
    daemon._speak_loop_once()                    # start reading (A or B)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.NEXT_SESSION})
    out = _spoken(daemon, speaker)
    assert any(t.startswith("Session changed:") for t in out)
    assert "A one." in out and "B one." in out   # both heard, nothing lost


def test_next_session_with_no_channels_speaks_cue():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")   # no prose -> no channels yet
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.NEXT_SESSION})
    out = _spoken(daemon, speaker)
    assert "No session." in out


def test_next_session_revisit_read_session_says_reading_again():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    # give folders so the announcement names them
    for s, cwd in (("A", "/u/alpha"), ("B", "/u/beta")):
        daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_START,
                               "session": s, "cwd": cwd, "plugin_version": ""})
    daemon.handle_message(_prose("A", "A one. ", 0, True))
    daemon.handle_message(_prose("B", "B one. ", 0, True))
    # earcon_only policy means only fg (B) would be read automatically; authorize A
    # to drain so both channels reach caught_up before we test NEXT_SESSION "reading again".
    daemon.router.channel("A").turn_done = True
    daemon.router._replay_authorized.add("A")
    _spoken(daemon, speaker, 12)                  # drain both -> both read
    speaker.spoken.clear()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.NEXT_SESSION})
    out = _spoken(daemon, speaker, 6)
    assert any("reading again" in t for t in out)


def test_next_session_is_debounced():
    from sonara.protocol import MsgType
    daemon = make_daemon()[0]
    assert daemon._debounce_suppress(MsgType.NEXT_SESSION, 1.0) is False
    assert daemon._debounce_suppress(MsgType.NEXT_SESSION, 1.10) is True   # rapid repeat dropped
