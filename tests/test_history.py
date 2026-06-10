from sonari.history import SessionHistory


def test_record_and_last_message_groups_by_message_boundary():
    h = SessionHistory()
    h.record("s1", "prose", "First sentence.")
    h.record("s1", "prose", "Second sentence.")
    h.end_message("s1")
    h.record("s1", "prose", "Next message.")
    assert [e.text for e in h.last_message("s1")] == ["Next message."]


def test_last_message_returns_whole_group():
    h = SessionHistory()
    h.record("s1", "prose", "A.")
    h.record("s1", "prose", "B.")
    h.end_message("s1")
    assert [e.text for e in h.last_message("s1")] == ["A.", "B."]


def test_last_message_empty_session():
    h = SessionHistory()
    assert h.last_message("nope") == []


def test_unheard_until_marked():
    h = SessionHistory()
    e1 = h.record("s1", "prose", "A.")
    e2 = h.record("s1", "prose", "B.")
    assert [e.text for e in h.unheard("s1")] == ["A.", "B."]
    e1.heard = True
    assert [e.text for e in h.unheard("s1")] == ["B."]
    e2.heard = True
    assert h.unheard("s1") == []


def test_reset_drops_session():
    h = SessionHistory()
    h.record("s1", "prose", "A.")
    h.reset("s1")
    assert h.last_message("s1") == []
    assert h.unheard("s1") == []


def test_rolling_cap_bounds_memory():
    h = SessionHistory(cap=3)
    for i in range(10):
        h.record("s1", "prose", "S{0}.".format(i))
    texts = [e.text for e in h.unheard("s1")]
    assert texts == ["S7.", "S8.", "S9."]


def test_other_session_with_unheard_most_recent_first():
    h = SessionHistory()
    h.record("a", "prose", "A1.")
    h.record("b", "prose", "B1.")          # b touched most recently
    assert h.other_session_with_unheard("fg") == "b"
    for e in h.unheard("b"):
        e.heard = True
    assert h.other_session_with_unheard("fg") == "a"
    for e in h.unheard("a"):
        e.heard = True
    assert h.other_session_with_unheard("fg") is None


def test_other_session_excludes_the_given_session():
    h = SessionHistory()
    h.record("fg", "prose", "X.")
    assert h.other_session_with_unheard("fg") is None
