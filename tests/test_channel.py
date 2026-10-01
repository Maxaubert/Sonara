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


# --- mutation API (#137): the daemon edits channels only through these ---

def _seeded():
    ch = SessionChannel("s")
    ch.seed(_item("old digest"))
    return ch


def test_seed_is_a_heard_replay_only_placeholder():
    ch = _seeded()
    assert ch.seeded is True and ch.turn_done is True
    assert ch.pending() == 0 and [it.text for it in ch.items] == ["old digest"]


def test_insert_at_replaces_the_seed_so_a_later_append_keeps_it():
    # The seeded-channel wipe (#137): content inserted into a seeded channel
    # used to leave seeded=True, so the NEXT append wiped the unread content.
    ch = _seeded()
    ch.insert_at(len(ch.items), [_item("Short answer here.")])
    assert ch.seeded is False
    assert [it.text for it in ch.items[ch.cursor:]] == ["Short answer here."]
    ch.append(_item("later"))
    assert [it.text for it in ch.items[ch.cursor:]] == ["Short answer here.", "later"]


def test_insert_at_bumps_gen_ends_replay_and_tracks_decisions():
    ch = SessionChannel("s")
    ch.replaying = True
    gen = ch.gen
    ch.insert_at(0, [_item("q?", is_decision=True), _item("a")])
    assert ch.gen > gen and ch.replaying is False
    assert ch.has_decision is True
    assert [it.text for it in ch.items] == ["q?", "a"]


def test_insert_at_never_lands_before_the_cursor():
    ch = SessionChannel("s")
    ch.append(_item("heard")); ch.next()
    ch.insert_at(0, [_item("next")])
    assert ch.next().text == "next"


def test_insert_at_with_nothing_is_a_no_op():
    ch = _seeded()
    gen = ch.gen
    ch.insert_at(0, [])
    assert ch.seeded is True and ch.gen == gen


def test_truncate_pending_returns_dropped_and_recomputes_decision():
    ch = SessionChannel("s")
    ch.append(_item("heard")); ch.next()
    ch.append(_item("q?", is_decision=True)); ch.append(_item("b"))
    dropped = ch.truncate_pending()
    assert [it.text for it in dropped] == ["q?", "b"]
    assert [it.text for it in ch.items] == ["heard"] and ch.pending() == 0
    assert ch.has_decision is False


def test_skip_to_end_consumes_pending_without_deleting():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.append(_item("q?", is_decision=True))
    skipped = ch.skip_to_end()
    assert [it.text for it in skipped] == ["a", "q?"]
    assert ch.caught_up() and len(ch.items) == 2 and ch.has_decision is False


def test_rewind_steps_back_one_item():
    ch = SessionChannel("s")
    ch.append(_item("a")); ch.next()
    assert ch.rewind() is True and ch.next().text == "a"
    ch2 = SessionChannel("s")
    assert ch2.rewind() is False


def test_unconsume_removes_the_just_spoken_item():
    ch = SessionChannel("s")
    a = _item("a")
    ch.append(a); ch.append(_item("b")); ch.next()
    assert ch.unconsume(a) is True
    assert [it.text for it in ch.items] == ["b"] and ch.cursor == 0
    assert ch.unconsume(a) is False


def test_remove_pending_matches_only_unread_items():
    ch = SessionChannel("s")
    ch.append(_item("x")); ch.next()
    ch.append(_item("x")); ch.append(_item("y"))
    removed = ch.remove_pending(lambda it: it.text == "x")
    assert [it.text for it in removed] == ["x"]
    assert [it.text for it in ch.items] == ["x", "y"] and ch.cursor == 1
