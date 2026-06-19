"""Pin-toggle hotkey: pin the current session's voice; toggle again to unpin."""
from sonari.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _prose(session, delta, index, final):
    return {
        "v": PROTOCOL_VERSION,
        "type": MsgType.PROSE,
        "session": session,
        "delta": delta,
        "index": index,
        "final": final,
    }


def test_pin_toggle_pins_current_and_speaks_folder():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    sessions.set_foreground("fg", cwd="/home/me/myapp")
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})
    assert sessions.pinned() == "fg"
    daemon._speak_loop_once()
    assert speaker.spoken == ["Pinned myapp."]


def test_pin_toggle_again_unpins_and_says_auto():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    sessions.set_foreground("fg", cwd="/home/me/myapp")
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})   # pin
    daemon._speak_loop_once()
    speaker.spoken.clear()
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})   # unpin
    assert sessions.pinned() is None
    daemon._speak_loop_once()
    assert speaker.spoken == ["Auto."]


def test_pinned_session_keeps_voice_when_another_submits():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})  # pin fg
    daemon.handle_message({"type": "set_foreground", "session": "bg"})
    assert sessions.foreground() == "fg"
    assert sessions.is_foreground("fg") is True
    assert sessions.is_foreground("bg") is False


def test_pinned_session_end_falls_back_to_auto():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})
    daemon.handle_message({"type": "session_end", "session": "fg"})
    assert sessions.pinned() is None
    assert sessions.foreground() is None


def test_set_foreground_message_carries_cwd_into_announcement():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground=None)
    daemon.handle_message({"type": "set_foreground", "session": "s1", "cwd": "/x/proj"})
    daemon.handle_message({"type": "pin_toggle", "session": "s1"})
    daemon._speak_loop_once()
    assert speaker.spoken == ["Pinned proj."]


def test_pin_toggle_with_no_session_beeps_error_only():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground=None)
    daemon.handle_message({"type": "pin_toggle", "session": ""})
    assert sessions.pinned() is None
    assert speaker.earcons == ["error"]      # only the error earcon, nothing else
    assert speaker.spoken == []


def test_pinned_session_blocks_background_from_being_served():
    """While fg is pinned, the router refuses to serve bg even when bg has items.
    Prose still lands in bg's channel (the channel architecture stores everything),
    but _pick() returns fg only, so _speak_loop_once never speaks bg's text."""
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="fg")
    daemon.handle_message({"type": "pin_toggle", "session": "fg"})     # pin fg
    daemon._speak_loop_once()                  # drain the "Pinned." cue
    speaker.spoken.clear()
    daemon.handle_message({"type": "set_foreground", "session": "bg"})  # bg submits a prompt
    daemon.handle_message(_prose("bg", "Background sentence here. ", 0, True))
    daemon.handle_message(_prose("fg", "Foreground sentence here. ", 0, True))
    daemon._speak_loop_once()
    # fg is pinned: the router serves fg, not bg
    assert speaker.spoken == ["Foreground sentence here."]


def test_repin_replays_pinned_channel_from_start():
    daemon, queue, speaker, sessions, _ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "One. Two. ", 0, True))
    daemon._speak_loop_once(); daemon._speak_loop_once()   # reads One., Two.
    speaker.spoken.clear()
    sessions.set_foreground("A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PIN_TOGGLE})  # pin A
    daemon._speak_loop_once()
    assert speaker.spoken[-1] == "One."     # replayed from the start
