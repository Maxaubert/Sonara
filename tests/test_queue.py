from echo.queue import SpeechItem, SpeechQueue


def _item(id, session="s1", kind="prose", text="t", is_decision=False):
    return SpeechItem(id=id, session=session, kind=kind, text=text, is_decision=is_decision)


def test_enqueue_then_pop_next_is_fifo():
    q = SpeechQueue()
    q.enqueue(_item(1, text="first"))
    q.enqueue(_item(2, text="second"))
    q.enqueue(_item(3, text="third"))
    assert q.pop_next().text == "first"
    assert q.pop_next().text == "second"
    assert q.pop_next().text == "third"


def test_pop_next_on_empty_returns_none():
    q = SpeechQueue()
    assert q.pop_next() is None


def test_len_tracks_pending_items():
    q = SpeechQueue()
    assert len(q) == 0
    q.enqueue(_item(1))
    q.enqueue(_item(2))
    assert len(q) == 2
    q.pop_next()
    assert len(q) == 1


def test_jump_to_decision_drops_leading_non_decision_keeps_decision_at_front():
    q = SpeechQueue()
    q.enqueue(_item(1, kind="prose", text="p1", is_decision=False))
    q.enqueue(_item(2, kind="prose", text="p2", is_decision=False))
    q.enqueue(_item(3, kind="choice", text="decide", is_decision=True))
    q.enqueue(_item(4, kind="prose", text="after", is_decision=False))
    q.jump_to_decision()
    assert len(q) == 2
    first = q.pop_next()
    assert first.text == "decide"
    assert first.is_decision is True
    assert q.pop_next().text == "after"


def test_jump_to_decision_with_no_decision_empties_queue():
    q = SpeechQueue()
    q.enqueue(_item(1, is_decision=False))
    q.enqueue(_item(2, is_decision=False))
    q.jump_to_decision()
    assert len(q) == 0
    assert q.pop_next() is None


def test_jump_to_decision_on_empty_is_noop():
    q = SpeechQueue()
    q.jump_to_decision()
    assert len(q) == 0


def test_clear_empties_queue():
    q = SpeechQueue()
    q.enqueue(_item(1))
    q.enqueue(_item(2))
    q.clear()
    assert len(q) == 0
    assert q.pop_next() is None
