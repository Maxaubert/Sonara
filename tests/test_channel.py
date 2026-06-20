from sonara.channel import SessionChannel
from sonara.queue import SpeechItem


def _item(text, is_decision=False):
    return SpeechItem(id=0, session="s", kind="prose", text=text, is_decision=is_decision)


def test_append_increases_pending_and_keeps_items():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.append(_item("b"))
    assert ch.pending() == 2 and len(ch.items) == 2 and ch.cursor == 0


def test_next_advances_cursor_without_discarding():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.append(_item("b"))
    assert ch.next().text == "a"
    assert ch.cursor == 1 and len(ch.items) == 2   # item retained for replay
    assert ch.next().text == "b"
    assert ch.next() is None                        # caught up


def test_ready_respects_minqueue_until_turn_done():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.append(_item("b"))
    assert ch.ready(3) is False        # below threshold, turn not done
    ch.turn_done = True
    assert ch.ready(3) is True         # turn done -> flush remainder
    ch.turn_done = False
    ch.append(_item("c"))
    assert ch.ready(3) is True         # reached threshold


def test_ready_true_for_decision_below_threshold():
    ch = SessionChannel("s")
    ch.append(_item("Question?", is_decision=True))
    assert ch.has_decision is True
    assert ch.ready(5) is True         # decisions are readable immediately


def test_reset_replays_from_start():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.append(_item("b"))
    ch.next(); ch.next()
    assert ch.caught_up() is True
    ch.reset()
    assert ch.cursor == 0 and ch.pending() == 2 and ch.next().text == "a"


def test_wipe_clears_everything():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.turn_done = True; ch.next()
    ch.wipe()
    assert ch.items == [] and ch.cursor == 0 and ch.turn_done is False
    assert ch.has_decision is False
