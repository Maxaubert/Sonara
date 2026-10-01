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


def test_reset_drops_session():
    h = SessionHistory()
    h.record("s1", "prose", "A.")
    h.reset("s1")
    assert h.last_message("s1") == []
    assert h.message_ids("s1") == []


def test_rolling_cap_bounds_memory():
    h = SessionHistory(cap=3)
    for i in range(10):
        h.record("s1", "prose", "S{0}.".format(i))
    texts = [e.text for e in h.last_message("s1")]
    assert texts == ["S7.", "S8.", "S9."]


def test_eviction_excludes_truncated_head_group_from_navigation():
    # #8: when the rolling cap evicts the HEAD of an older message group, Up must
    # not replay the surviving fragment -- message_ids excludes the truncated group.
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


def test_removed_backlog_helpers_are_gone():
    # DC1/DC4: catch-up's unheard backlog and nth_last_message stepping into
    # older messages were removed (one message, always the last).
    h = SessionHistory()
    for name in ("unheard", "other_session_with_unheard", "nth_last_message"):
        assert not hasattr(h, name)
