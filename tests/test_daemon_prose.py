from echo.protocol import MsgType, PROTOCOL_VERSION
from echo.queue import SpeechItem
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
