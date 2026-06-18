from sonari.protocol import MsgType, PROTOCOL_VERSION
from sonari.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _flush(session):
    return {"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": session}


def _prose(session, delta, index, final):
    return {
        "v": PROTOCOL_VERSION,
        "type": MsgType.PROSE,
        "session": session,
        "delta": delta,
        "index": index,
        "final": final,
    }


def test_prose_from_non_foreground_session_is_dropped():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    out = daemon.handle_message(_prose("other", "Hello there. ", 0, False))
    assert out is None
    assert len(queue) == 0


def test_prose_from_foreground_enqueues_one_item_per_chunk():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # Two complete sentences -> two chunks -> two enqueued items.
    daemon.handle_message(_prose("fg", "Hello there. How are you? ", 0, False))
    assert len(queue) == 2
    first = queue.pop_next()
    second = queue.pop_next()
    assert isinstance(first, SpeechItem)
    assert first.session == "fg"
    assert first.kind == "prose"
    assert first.is_decision is False
    assert first.text == "Hello there."
    assert second.text == "How are you?"
    # ids are unique and increasing
    assert second.id > first.id


def test_prose_partial_then_final_flushes_remainder():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # Partial sentence (no terminator) -> no chunk yet.
    daemon.handle_message(_prose("fg", "tail with no period", 0, False))
    assert len(queue) == 0
    # final=True flushes the remainder as one chunk.
    daemon.handle_message(_prose("fg", "", 1, True))
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.text == "tail with no period"


def test_prose_uses_per_session_assembler():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # Same index reused across sessions must NOT be deduped across sessions.
    daemon.handle_message(_prose("fg", "Foreground sentence here. ", 0, False))
    # background session at index 0 is dropped (not foreground) but must not crash
    daemon.handle_message(_prose("bg", "Background sentence here. ", 0, False))
    assert len(queue) == 1
    assert queue.pop_next().text == "Foreground sentence here."


def test_prose_enqueued_at_verbosity_everything():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg", verbosity="everything")
    daemon.handle_message(_prose("fg", "Hello world. ", 0, False))
    assert len(queue) == 1


def test_prose_enqueued_at_verbosity_medium():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg", verbosity="medium")
    daemon.handle_message(_prose("fg", "Hello world. ", 0, False))
    assert len(queue) == 1


def test_prose_dropped_at_verbosity_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg", verbosity="quiet")
    daemon.handle_message(_prose("fg", "Hello world. ", 0, False))
    assert len(queue) == 0


def _earcon(session, kind):
    return {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "session": session, "kind": kind}


def test_owner_keeps_voice_across_interchunk_drain_when_other_session_flips_foreground():
    """H1: between streamed chunks of ONE reply the queue drains to 0. If another
    session flips foreground in that gap, the original owner must KEEP the voice and
    its remaining deltas must still be enqueued (not captured to history and lost)."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    # A streams its first sentence -> A acquires the voice, message is 'open'.
    daemon.handle_message(_prose("A", "First sentence here. ", 0, False))
    assert len(queue) == 1
    assert daemon._voice_owner == "A"
    # The speak loop drains A's only queued item: queue hits 0 mid-message.
    daemon._speak_loop_once()
    assert len(queue) == 0
    # A still owns the voice because its message is still open (no final yet).
    assert daemon._voice_owner == "A"
    # Now a SECOND session flips foreground (new tab / other window submits).
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "B"})
    # A's next delta must STILL be enqueued — the reply does not go silent.
    daemon.handle_message(_prose("A", "Second sentence here. ", 1, False))
    assert len(queue) == 1
    assert queue.pop_next().text == "Second sentence here."


def test_open_message_released_at_turn_boundary():
    """Ownership is held during an open message but released once the turn ends
    (PROSE final or the Stop turn_done earcon), so a new foreground session can
    then acquire a free voice."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Hello there. ", 0, False))
    daemon._speak_loop_once()                      # drain; A keeps voice (open msg)
    assert daemon._voice_owner == "A"
    # Turn ends via the Stop turn_done earcon (carries the session).
    daemon.handle_message(_earcon("A", "turn_done"))
    daemon._speak_loop_once()                       # empty branch now releases owner
    assert daemon._voice_owner is None


def test_flush_resets_assembler_so_next_turn_is_clean():
    """After FLUSH, stale assembler state (_seen/_buf/_pending) must not leak.

    Scenario:
      1. Feed a partial (no terminator) at index 0  -> nothing enqueued yet.
      2. FLUSH the session              -> queue cleared, assembler dropped.
      3. Feed a *new* final message at index 0 (same index, fresh turn).
         The assembler must NOT treat it as a duplicate (old _seen), and
         the new content (not the old partial) must be enqueued.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")

    # Step 1: partial delta – no sentence terminator, nothing enqueued.
    daemon.handle_message(_prose("fg", "old partial content", 0, False))
    assert len(queue) == 0

    # Step 2: FLUSH – clears queue items and drops the assembler.
    daemon.handle_message(_flush("fg"))
    assert len(queue) == 0
    assert "fg" not in daemon._assemblers

    # Step 3: fresh final message re-using index 0 (new turn, new assembler).
    daemon.handle_message(_prose("fg", "New sentence here.", 0, True))
    assert len(queue) == 1
    item = queue.pop_next()
    assert item.text == "New sentence here."
