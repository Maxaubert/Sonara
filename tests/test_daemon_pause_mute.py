"""Pause (global full-silence halt) + per-session mute on the active reader."""
from tests.daemon_helpers import make_daemon
from sonari.protocol import MsgType, PROTOCOL_VERSION


def _prose(s, d, i, f):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": s,
            "delta": d, "index": i, "final": f}


# ---------------------------------------------------------------------------
# MUTE tests
# ---------------------------------------------------------------------------

def test_global_mute_silences_all_sessions():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "A one. ", 0, True))
    daemon.handle_message(_prose("B", "B one. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})   # global mute
    assert daemon._muted is True
    daemon._speak_loop_once()                        # "Muted." cue (mute_exempt) is heard
    assert speaker.spoken == ["Muted."]
    speaker.spoken.clear()
    for _ in range(4):
        daemon._speak_loop_once()                    # both sessions' prose is dropped
    assert speaker.spoken == []                       # all silent, not just the active one


def test_global_mute_toggles():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert daemon._muted is True
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert daemon._muted is False


def test_mute_cancels_currently_speaking_item():
    """MUTE cancels the speaker so the live utterance stops immediately."""
    from sonari.queue import SpeechItem
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon._current_item = SpeechItem(id=1, session="A", kind="prose",
                                      text="mid-utterance", is_decision=False)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    assert daemon._muted is True
    assert speaker.cancels >= 1


def test_mute_confirmation_is_heard_despite_mute():
    """'Muted.' uses mute_exempt so it plays even though everything else is silenced."""
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    daemon._speak_loop_once()
    assert "Muted." in speaker.spoken


def test_unmute_confirmation_is_spoken():
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})  # mute
    daemon._speak_loop_once()                  # "Muted."
    speaker.spoken.clear()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})  # unmute
    daemon._speak_loop_once()
    assert speaker.spoken == ["Unmuted."]


def test_pause_cue_heard_with_no_session():
    """Pause confirms audibly even when NO session is registered (fg/active None):
    the cue routes to the CONTROL channel, which the loop still voices."""
    daemon, queue, speaker, *_ = make_daemon(foreground=None)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
    daemon._speak_loop_once()
    assert "Paused." in speaker.spoken


def test_mute_cue_heard_with_no_session():
    daemon, queue, speaker, *_ = make_daemon(foreground=None)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.MUTE})
    daemon._speak_loop_once()
    assert "Muted." in speaker.spoken


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


# ---------------------------------------------------------------------------
# Mid-utterance PAUSE concurrency tests (Task 5 pass 2)
# ---------------------------------------------------------------------------

def test_paused_cue_spoken_after_mid_utterance_pause():
    """Repro: PAUSE arrives while speak() is running (item already consumed from
    channel); speak() returns False+paused, cursor rewinds, 'Paused.' is now at
    cursor+1 — the paused branch must scan beyond the cursor to find and speak it.
    """
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Item one. Item two. ", 0, True))

    # Arm the speaker to simulate mid-utterance PAUSE:
    # While "Item one." is being spoken, PAUSE arrives (sets _paused, cancels
    # speaker, inserts "Paused." at cursor). speak() then returns False.
    original_speak = speaker.speak

    def speak_that_pauses(text, cancel_epoch=None):
        if text == "Item one.":
            # PAUSE arrives mid-utterance
            daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
            return False   # cancelled by pause
        return original_speak(text, cancel_epoch=cancel_epoch)

    speaker.speak = speak_that_pauses

    # This _speak_loop_once consumes "Item one." via ch.next(), calls speak()
    # which fires PAUSE (inserts "Paused." at cursor, rewinds to cursor-1),
    # gets False back, sees _paused → rewinds cursor again. Net: cursor points
    # at "Item one.", "Paused." is at cursor+1.
    daemon._speak_loop_once()

    assert daemon._paused.is_set()
    # "Item one." itself was not completed (False) so it's NOT in spoken.
    # The paused branch must now find and speak "Paused." even though it's
    # past the cursor.
    speaker.spoken.clear()
    daemon._speak_loop_once()
    assert "Paused." in speaker.spoken, (
        "Expected 'Paused.' to be spoken while paused, but it was not. "
        f"Spoken: {speaker.spoken}"
    )

    # Further iterations while paused should hold (no new items spoken).
    speaker.spoken.clear()
    daemon._speak_loop_once()
    assert speaker.spoken == [], f"Expected silence while paused, got: {speaker.spoken}"


def test_mid_utterance_pause_rewinds_and_resumes_interrupted_item():
    """When PAUSE interrupts an utterance, the cursor must rewind so the
    interrupted item is not lost; on resume, it replays from the start."""
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Alpha. Beta. ", 0, True))

    original_speak = speaker.speak

    def speak_that_pauses(text, cancel_epoch=None):
        if text == "Alpha.":
            daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
            return False
        return original_speak(text, cancel_epoch=cancel_epoch)

    speaker.speak = speak_that_pauses

    # Run the loop to trigger the mid-utterance pause on "Alpha."
    daemon._speak_loop_once()

    assert daemon._paused.is_set()

    # Drain "Paused." cue (may be there or not depending on order; just clear it)
    speaker.speak = original_speak
    daemon._speak_loop_once()   # speaks "Paused." or holds

    # Resume
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
    assert not daemon._paused.is_set()
    daemon._speak_loop_once()   # "Resumed."
    daemon._speak_loop_once()   # "Alpha." (rewound)
    assert "Alpha." in speaker.spoken, (
        f"Expected 'Alpha.' to replay after resume but got: {speaker.spoken}"
    )


def test_pause_replay_preserves_heard_marker():
    """The interrupted item's _pending_heard entry must survive the cursor rewind
    so that when it is eventually spoken (completed), its history entry is marked
    heard."""
    daemon, queue, speaker, *_ = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Marked. ", 0, True))

    ch = daemon.router.channel("A")
    # Confirm a pending_heard entry exists for the item before it's spoken
    assert len(daemon._pending_heard) == 1
    item_id = list(daemon._pending_heard.keys())[0]

    original_speak = speaker.speak

    def speak_that_pauses(text, cancel_epoch=None):
        if text == "Marked.":
            daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PAUSE})
            return False
        return original_speak(text, cancel_epoch=cancel_epoch)

    speaker.speak = speak_that_pauses
    daemon._speak_loop_once()   # triggers mid-utterance pause on "Marked."

    # The _pending_heard entry must NOT have been removed (item was not completed)
    assert item_id in daemon._pending_heard, (
        "Interrupted item's heard-marker was wrongly dropped on cursor rewind."
    )
