"""min-queue batching: prose is held in a per-session buffer until it reaches the
configured threshold (or the turn ends), then flushed to the speech queue."""
from sonari.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _prose(session, delta, index, final):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": session,
            "delta": delta, "index": index, "final": final}


def _drain(queue):
    out = []
    while True:
        it = queue.pop_next()
        if it is None:
            return out
        out.append(it.text)


def test_prose_held_below_threshold_then_flushes_all_at_once():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 3
    # two sentences: below the threshold -> nothing queued yet
    daemon.handle_message(_prose("fg", "One. Two. ", 0, False))
    assert len(queue) == 0
    # the third reaches the threshold -> all three flush together
    daemon.handle_message(_prose("fg", "Three. ", 1, False))
    assert _drain(queue) == ["One.", "Two.", "Three."]


def test_turn_end_flushes_sub_threshold_remainder():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 3
    # one sentence, but the turn finishes (final) -> read it anyway
    daemon.handle_message(_prose("fg", "Only one. ", 0, True))
    assert _drain(queue) == ["Only one."]


def test_turn_done_earcon_flushes_buffer_when_final_never_arrives():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 5
    daemon.handle_message(_prose("fg", "Held one. Held two. ", 0, False))
    assert len(queue) == 0
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
    assert len(queue) == 0                        # prose buffered, below threshold
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.TOOL,
                           "session": "fg", "tool": "Bash", "summary": "ls"})
    assert _drain(queue) == ["Looking now.", "ls"]   # prose first, then the tool cue


def test_new_prompt_flush_discards_buffered_prose():
    daemon, queue, speaker, *_ = make_daemon(foreground="fg")
    daemon.config["minqueue"] = 3
    daemon.handle_message(_prose("fg", "One. Two. ", 0, False))
    assert len(queue) == 0                   # buffered, not queued
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH,
                           "session": "fg"})
    assert len(queue) == 0
    assert daemon._prose_buffer.get("fg", []) == []   # buffer dropped
