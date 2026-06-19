"""min-queue batching: prose is held in a per-session channel until it reaches the
configured threshold (or the turn ends), then flushed to the speech router."""
from sonari.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _prose(session, delta, index, final):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": session,
            "delta": delta, "index": index, "final": final}


def _drain(queue):
    """Drain all *currently readable* items from the router (respects minqueue)."""
    out = []
    while True:
        it = queue.pop_next()
        if it is None:
            return out
        out.append(it.text)


def _channel_pending(daemon, session="fg"):
    """Items sitting in the channel (may be held by minqueue)."""
    return daemon.router.channel(session).pending()


def test_prose_held_below_threshold_then_flushes_all_at_once():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 3
    # two sentences: below the threshold -> nothing readable yet
    daemon.handle_message(_prose("fg", "One. Two. ", 0, False))
    assert _drain(queue) == []                # held: router won't produce items yet
    assert _channel_pending(daemon) == 2      # items ARE in the channel
    # the third reaches the threshold -> all three flush together
    daemon.handle_message(_prose("fg", "Three. ", 1, False))
    assert _drain(queue) == ["One.", "Two.", "Three."]


def _turn_done(session):
    return {"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
            "kind": "turn_done", "session": session}


def test_turn_boundary_flushes_sub_threshold_remainder():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 3
    # A message ending (final) is NOT the turn boundary — Claude Code marks each
    # text block final, and there are many per reply. The remainder is read when
    # the TURN ends (turn_done), not at every block.
    daemon.handle_message(_prose("fg", "Only one. ", 0, True))
    assert _drain(queue) == []                # held: final alone doesn't flush at minqueue=3
    daemon.handle_message(_turn_done("fg"))
    assert _drain(queue) == ["Only one."]


def test_multiple_final_messages_batch_across_the_whole_turn():
    # The bug: a tool-heavy reply streams several text blocks, each marked final.
    # Below-threshold prose must accumulate ACROSS the blocks and only read once
    # the turn ends — not once per block.
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 5
    daemon.handle_message(_prose("fg", "Alpha one. Alpha two. ", 0, True))
    assert _drain(queue) == []                # block 1 done, still held
    daemon.handle_message(_prose("fg", "Beta one. Beta two. ", 0, True))
    assert _drain(queue) == []                # block 2 done, still below 5
    daemon.handle_message(_turn_done("fg"))
    assert _drain(queue) == ["Alpha one.", "Alpha two.", "Beta one.", "Beta two."]


def test_turn_done_earcon_flushes_buffer_when_final_never_arrives():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 5
    daemon.handle_message(_prose("fg", "Held one. Held two. ", 0, False))
    assert _drain(queue) == []
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    assert _drain(queue) == ["Held one.", "Held two."]


def test_minqueue_one_reads_immediately():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")   # default minqueue == 1
    daemon.handle_message(_prose("fg", "Hi there. ", 0, False))
    assert _drain(queue) == ["Hi there."]


def test_tool_announcement_bypasses_the_buffer():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 5
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.TOOL,
                           "session": "fg", "tool": "Bash", "summary": "ls"})
    assert _drain(queue) == ["ls"]          # immediate despite a high threshold


def test_immediate_cue_flushes_buffered_prose_first_to_keep_order():
    # A tool cue is immediate, but prose that came BEFORE it must be read first —
    # the cue must not jump ahead of buffered prose.
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 5
    daemon.handle_message(_prose("fg", "Looking now. ", 0, False))
    assert _drain(queue) == []                        # prose held, below threshold
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.TOOL,
                           "session": "fg", "tool": "Bash", "summary": "ls"})
    assert _drain(queue) == ["Looking now.", "ls"]   # prose first, then the tool cue


def test_new_prompt_flush_discards_buffered_prose():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 3
    daemon.handle_message(_prose("fg", "One. Two. ", 0, False))
    assert _drain(queue) == []             # held, not readable
    assert _channel_pending(daemon) == 2   # items are in the channel
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH,
                           "session": "fg"})
    # After FLUSH the channel is wiped — no items remain
    assert _channel_pending(daemon) == 0
    assert _drain(queue) == []             # nothing readable or held
