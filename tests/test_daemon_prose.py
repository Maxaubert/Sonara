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


def test_prose_from_non_foreground_session_is_captured_not_spoken():
    # In the per-session-channel model ALL sessions get items in their channel;
    # the ROUTER gates who actually speaks (foreground/pinned only).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_prose("other", "Hello there. ", 0, False))
    # "other" is not foreground: its item is captured in "other"'s channel.
    assert daemon.router.channel("other").pending() == 1
    # The router will not pick "other" to speak (only "fg" is foreground).
    assert daemon.router.channel("fg").pending() == 0


def test_prose_from_foreground_enqueues_one_item_per_chunk():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # Two complete sentences -> two chunks -> two items in fg's channel.
    daemon.handle_message(_prose("fg", "Hello there. How are you? ", 0, False))
    ch = daemon.router.channel("fg")
    assert ch.pending() == 2
    first = ch.items[ch.cursor]
    second = ch.items[ch.cursor + 1]
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
    assert daemon.router.channel("fg").pending() == 0
    # final=True flushes the remainder as one chunk.
    daemon.handle_message(_prose("fg", "", 1, True))
    ch = daemon.router.channel("fg")
    assert ch.pending() == 1
    item = ch.items[ch.cursor]
    assert item.text == "tail with no period"


def test_prose_uses_per_session_assembler():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    # Same index reused across sessions must NOT be deduped across sessions.
    daemon.handle_message(_prose("fg", "Foreground sentence here. ", 0, False))
    # background session at index 0 goes to "bg"'s own channel (captured, not spoken)
    daemon.handle_message(_prose("bg", "Background sentence here. ", 0, False))
    # fg has 1 pending item; bg also has 1 (captured) but router won't pick bg.
    assert daemon.router.channel("fg").pending() == 1
    assert daemon.router.channel("fg").items[0].text == "Foreground sentence here."
    # bg item is captured in its channel but will not be spoken unless bg becomes fg
    assert daemon.router.channel("bg").pending() == 1


def test_prose_enqueued_at_verbosity_everything():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg", verbosity="everything")
    daemon.handle_message(_prose("fg", "Hello world. ", 0, False))
    assert daemon.router.channel("fg").pending() == 1


def test_prose_enqueued_at_verbosity_medium():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg", verbosity="medium")
    daemon.handle_message(_prose("fg", "Hello world. ", 0, False))
    assert daemon.router.channel("fg").pending() == 1


def test_prose_dropped_at_verbosity_quiet():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg", verbosity="quiet")
    daemon.handle_message(_prose("fg", "Hello world. ", 0, False))
    assert daemon.router.channel("fg").pending() == 0


def _earcon(session, kind):
    return {"v": PROTOCOL_VERSION, "type": MsgType.EARCON, "session": session, "kind": kind}


def test_owner_keeps_voice_across_interchunk_drain_when_other_session_flips_foreground():
    """H1: between streamed chunks of ONE reply the channel drains to 0. If another
    session flips foreground in that gap, the original session must KEEP the voice
    (router.active stays on A because its turn is still open) and its remaining
    deltas must still be captured in A's channel."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    # A streams its first sentence -> goes into A's channel, turn open.
    daemon.handle_message(_prose("A", "First sentence here. ", 0, False))
    assert daemon.router.channel("A").pending() == 1
    # The speak loop drains A's only item: channel hits 0 mid-message.
    daemon._speak_loop_once()
    assert daemon.router.channel("A").pending() == 0
    # router.active is set to "A" by next_item() called in _speak_loop_once
    assert daemon.router.active == "A"
    # Now a SECOND session flips foreground (new tab / other window submits).
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_FOREGROUND, "session": "B"})
    # A's next delta must STILL be captured in A's channel.
    daemon.handle_message(_prose("A", "Second sentence here. ", 1, False))
    assert daemon.router.channel("A").pending() == 1
    assert daemon.router.channel("A").items[daemon.router.channel("A").cursor].text == "Second sentence here."


def test_open_message_released_at_turn_boundary():
    """Ownership is held during an open message (router.active == A) but released
    once the turn ends (PROSE final or the Stop turn_done earcon), so a new
    foreground session can then acquire the voice."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="A")
    daemon.handle_message(_prose("A", "Hello there. ", 0, False))
    daemon._speak_loop_once()                      # drain; A keeps voice (open msg)
    assert daemon.router.active == "A"
    # Turn ends via the Stop turn_done earcon (carries the session).
    daemon.handle_message(_earcon("A", "turn_done"))
    daemon._speak_loop_once()                       # empty branch now: active -> None
    assert daemon.router.active is None


def test_flush_resets_assembler_so_next_turn_is_clean():
    """After FLUSH, stale assembler state (_seen/_buf/_pending) must not leak.

    Scenario:
      1. Feed a partial (no terminator) at index 0  -> nothing enqueued yet.
      2. FLUSH the session              -> channel cleared, assembler dropped.
      3. Feed a *new* final message at index 0 (same index, fresh turn).
         The assembler must NOT treat it as a duplicate (old _seen), and
         the new content (not the old partial) must be enqueued.
    """
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")

    # Step 1: partial delta – no sentence terminator, nothing enqueued.
    daemon.handle_message(_prose("fg", "old partial content", 0, False))
    assert daemon.router.channel("fg").pending() == 0

    # Step 2: FLUSH – clears channel items and drops the assembler.
    daemon.handle_message(_flush("fg"))
    assert daemon.router.channel("fg").pending() == 0
    assert "fg" not in daemon._assemblers

    # Step 3: fresh final message re-using index 0 (new turn, new assembler).
    daemon.handle_message(_prose("fg", "New sentence here.", 0, True))
    ch = daemon.router.channel("fg")
    assert ch.pending() == 1
    item = ch.items[ch.cursor]
    assert item.text == "New sentence here."
