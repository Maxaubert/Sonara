from sonara.history import SessionHistory


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


def test_eviction_excludes_truncated_head_group_from_navigation():
    # #8: when the rolling cap evicts the HEAD of an older message group, nav must
    # not replay the surviving fragment — message_ids excludes the truncated group.
    h = SessionHistory(cap=3)
    h.record("s", "prose", "a1")
    h.record("s", "prose", "a2")        # group 0: a1, a2
    h.end_message("s")
    h.record("s", "prose", "b1")
    h.record("s", "prose", "b2")        # group 1: b1, b2 -> evicts a1 (group 0 head)
    assert h.message_ids("s") == [1]    # truncated group 0 excluded
    assert [e.text for e in h.entries_for_message("s", 1)] == ["b1", "b2"]


def test_complete_groups_remain_navigable():
    # A group whose head is still present stays navigable (regression guard).
    h = SessionHistory(cap=10)
    h.record("s", "prose", "a1"); h.end_message("s")
    h.record("s", "prose", "b1"); h.end_message("s")
    assert h.message_ids("s") == [0, 1]


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


def test_nth_last_message_walks_back_through_groups():
    from sonara.history import SessionHistory
    h = SessionHistory()
    # group 0: two prose sentences
    h.record("s", "prose", "a1"); h.record("s", "prose", "a2"); h.end_message("s")
    # group 1: one choice
    h.record("s", "choice", "b1"); h.end_message("s")
    # group 2: current (open) prose
    h.record("s", "prose", "c1")
    assert [e.text for e in h.nth_last_message("s", 0)] == ["c1"]       # current
    assert [e.text for e in h.nth_last_message("s", 1)] == ["b1"]       # previous
    assert [e.text for e in h.nth_last_message("s", 2)] == ["a1", "a2"] # two back
    assert h.nth_last_message("s", 3) == []                            # out of range
    assert h.nth_last_message("s", -1) == []
    assert h.nth_last_message("missing", 0) == []


def test_message_ids_and_entries_for_message():
    from sonara.history import SessionHistory
    h = SessionHistory()
    h.record("s", "prose", "a1"); h.record("s", "prose", "a2"); h.end_message("s")
    h.record("s", "choice", "b1"); h.end_message("s")
    h.record("s", "prose", "c1")
    ids = h.message_ids("s")
    assert ids == [0, 1, 2]                                   # oldest -> newest
    assert [e.text for e in h.entries_for_message("s", 0)] == ["a1", "a2"]
    assert [e.text for e in h.entries_for_message("s", 1)] == ["b1"]
    assert h.message_ids("missing") == []
