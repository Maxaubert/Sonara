"""Pause (global full-silence halt) + per-session mute on the active reader."""
from tests.daemon_helpers import make_daemon
from sonari.protocol import MsgType, PROTOCOL_VERSION


def _prose(s, d, i, f):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s,
            "delta": d, "index": i, "final": f}


# ---------------------------------------------------------------------------
# MUTE tests
# ---------------------------------------------------------------------------

def test_mute_targets_active_reader_and_skips_it():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Secret one. Secret two. ", 0, True))
    daemon._speak_loop_once()                        # speaks "Secret one." -> A is active
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert daemon.router.channel("A").muted is True
    speaker.spoken.clear()
    daemon._speak_loop_once()                        # "Session muted." cue (mute_exempt)
    assert speaker.spoken == ["Session muted."]
    speaker.spoken.clear()
    daemon._speak_loop_once()                        # A still muted, "Secret two." skipped
    assert speaker.spoken == []


def test_mute_toggle_unmutes():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Hello. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert daemon.router.channel("A").muted is True
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert daemon.router.channel("A").muted is False


def test_mute_cancels_currently_speaking_item():
    """MUTE cancels the speaker when it arrives while that session's item is live."""
    from sonari.queue import SpeechItem
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    # Simulate an in-progress utterance by planting _current_item directly
    # (mirrors what _speak_loop_once sets before calling speaker.speak)
    fake_item = SpeechItem(id=1, session="A", kind="prose",
                           text="mid-utterance", is_decision=False)
    daemon._current_item = fake_item
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert daemon.router.channel("A").muted is True
    assert speaker.cancels >= 1


def test_mute_confirmation_is_heard_despite_mute():
    """'Session muted.' uses mute_exempt so it plays even though channel is muted."""
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    # channel is now muted, but mute-exempt cue should still play
    daemon._speak_loop_once()
    assert "Session muted." in speaker.spoken


def test_unmute_confirmation_is_spoken():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})  # mute
    daemon._speak_loop_once()                  # "Session muted."
    speaker.spoken.clear()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})  # unmute
    daemon._speak_loop_once()
    assert speaker.spoken == ["Session unmuted."]


# ---------------------------------------------------------------------------
# PAUSE tests
# ---------------------------------------------------------------------------

def test_pause_halts_then_resumes_same_item():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Alpha. Beta. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})  # pause
    # First loop_once speaks "Paused." (pause_exempt), not "Alpha."
    daemon._speak_loop_once()
    assert "Alpha." not in speaker.spoken          # normal item held
    # While paused, subsequent iterations hold
    speaker.spoken.clear()
    daemon._speak_loop_once()
    assert speaker.spoken == []                    # still held after cue consumed
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})  # resume
    daemon._speak_loop_once()
    assert "Resumed." in speaker.spoken
    speaker.spoken.clear()
    daemon._speak_loop_once()
    assert speaker.spoken == ["Alpha."]


def test_paused_cue_is_spoken_while_paused():
    """'Paused.' is pause_exempt so _speak_loop_once speaks it while paused,
    then holds all normal items."""
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Normal item. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
    assert daemon._paused.is_set()
    # First call: "Paused." (pause_exempt) is spoken
    daemon._speak_loop_once()
    assert speaker.spoken == ["Paused."]
    # Second call: no more pause_exempt items -> held, nothing extra spoken
    daemon._speak_loop_once()
    assert speaker.spoken == ["Paused."]           # unchanged; "Normal item." held


def test_pause_toggle_cancels_current():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    assert not daemon._paused.is_set()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
    assert daemon._paused.is_set() and speaker.cancels == 1
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
    assert not daemon._paused.is_set()


def test_resume_speaks_resumed_then_continues():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon._paused.set()
    daemon.handle_message(_prose("A", "Interrupted. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})  # resume
    assert not daemon._paused.is_set()
    daemon._speak_loop_once()
    assert speaker.spoken == ["Resumed."]
    daemon._speak_loop_once()
    assert speaker.spoken == ["Resumed.", "Interrupted."]


def test_pause_and_resume_cues_are_audible_even_when_muted():
    """Pause/resume cues use mute_exempt+pause_exempt so they are always heard."""
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    # Mute the session first
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    daemon._speak_loop_once()                      # "Session muted."
    speaker.spoken.clear()
    # Now pause: "Paused." should still be heard despite mute
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
    daemon._speak_loop_once()
    assert speaker.spoken == ["Paused."]           # heard despite mute
    # Resume: "Resumed." should be heard too
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
    daemon._speak_loop_once()
    assert speaker.spoken == ["Paused.", "Resumed."]


def test_new_prompt_clears_pause():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon._paused.set()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "A"})
    assert not daemon._paused.is_set()
