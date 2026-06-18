"""Alt+Down play/pause (global) and Alt+Up sticky per-session mute."""
from tests.daemon_helpers import make_daemon


def test_pause_toggle_cancels_current():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    assert not daemon._paused.is_set()
    daemon.handle_message({"type": "pause", "session": "fg"})
    assert daemon._paused.is_set() and speaker.cancels == 1
    daemon.handle_message({"type": "pause", "session": "fg"})
    assert not daemon._paused.is_set()


def test_speak_loop_holds_while_paused_then_resumes():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon._enqueue("fg", "prose", "hello", False)
    daemon._paused.set()
    daemon._wake.set()                    # so the pause wait returns at once
    daemon._speak_loop_once()
    assert speaker.spoken == [] and len(queue) == 1   # held, not consumed
    daemon._paused.clear()
    daemon._speak_loop_once()
    assert speaker.spoken == ["hello"]


def test_mute_drops_speech_but_unmute_resumes():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.handle_message({"type": "mute", "session": "fg"})
    assert "fg" in daemon._muted_sessions
    daemon._speak_loop_once()             # speak the "Session muted." confirmation
    speaker.spoken.clear()
    daemon._enqueue("fg", "prose", "secret", False)
    daemon._speak_loop_once()
    assert speaker.spoken == []           # real content: dropped, not spoken
    daemon.handle_message({"type": "mute", "session": "fg"})
    assert "fg" not in daemon._muted_sessions
    daemon._speak_loop_once()             # "Session unmuted."
    speaker.spoken.clear()
    daemon._enqueue("fg", "prose", "hello", False)
    daemon._speak_loop_once()
    assert speaker.spoken == ["hello"]


def test_muting_flushes_pending_user_content():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon._enqueue("fg", "prose", "queued", False)
    daemon.handle_message({"type": "mute", "session": "fg"})
    texts = []
    while True:
        it = queue.pop_next()
        if it is None:
            break
        texts.append(it.text)
    assert "queued" not in texts and texts == ["Session muted."]


def test_mute_speaks_muted_and_unmuted_confirmations():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.handle_message({"type": "mute", "session": "fg"})
    daemon._speak_loop_once()                 # "Session muted." is mute_exempt
    assert "Session muted." in speaker.spoken
    daemon._enqueue("fg", "prose", "secret", False)
    daemon._speak_loop_once()
    assert "secret" not in speaker.spoken     # real content still muted
    daemon.handle_message({"type": "mute", "session": "fg"})
    daemon._speak_loop_once()
    assert "Session unmuted." in speaker.spoken


def test_pause_during_speech_requeues_interrupted_item():
    # A pause landing WHILE an item is being spoken: speak() reports not-completed
    # (the speaker's cancel epoch aborted it) and the pause flag is set by then.
    # The speak loop must re-queue that exact item at the FRONT and clear the
    # current-item claim, so resume picks back up where it stopped.
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon._enqueue("fg", "prose", "interrupted sentence", False)

    def interrupted(text, cancel_epoch=None):
        speaker.spoken.append(text)
        daemon._paused.set()          # pause arrived mid-utterance
        return False                  # ... and cancelled it

    speaker.speak = interrupted
    daemon._speak_loop_once()
    assert speaker.spoken == ["interrupted sentence"]
    assert daemon._current_item is None
    assert len(queue) == 1 and queue.pop_next().text == "interrupted sentence"


def test_pause_replay_preserves_heard_marker():
    # Regression #10: an item interrupted by pause keeps its _pending_heard entry,
    # so when the replay completes it is still recorded as heard (not left unheard).
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    entry = daemon.history.record("fg", "prose", "hello")
    daemon._enqueue("fg", "prose", "hello", False, entry=entry)

    def interrupted(text, cancel_epoch=None):
        speaker.spoken.append(text)
        daemon._paused.set()
        return False

    speaker.speak = interrupted
    daemon._speak_loop_once()                       # interrupted by the pause
    assert entry.heard is False
    assert entry in daemon._pending_heard.values()  # entry preserved for the replay
    # resume and let the replay complete
    daemon._paused.clear()
    speaker.speak = lambda t, cancel_epoch=None: (speaker.spoken.append(t) or True)
    daemon._speak_loop_once()
    assert entry.heard is True


def test_flush_racing_a_paused_utterance_does_not_resurrect_it():
    """L2: the re-queue-on-pause check must be inside the lock. If a FLUSH lands
    between speak() returning not-completed and the re-queue (clearing pause and
    flushing the queue), the interrupted item must NOT be resurrected into the
    now-flushed queue."""
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon._enqueue("fg", "prose", "interrupted", False)

    def interrupted(text, cancel_epoch=None):
        speaker.spoken.append(text)
        daemon._paused.set()                       # pause arrived mid-utterance
        # ... but then a new prompt (FLUSH) races in and supersedes the pause.
        daemon.handle_message({"type": "flush", "session": "fg"})
        return False

    speaker.speak = interrupted
    daemon._speak_loop_once()
    assert not daemon._paused.is_set()             # FLUSH cleared the pause
    assert len(queue) == 0                          # item NOT resurrected
    assert daemon._current_item is None


def test_new_prompt_clears_pause():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon._paused.set()
    daemon.handle_message({"type": "flush", "session": "fg"})
    assert not daemon._paused.is_set()
