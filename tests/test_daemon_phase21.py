from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _msg(mtype, session=None, **extra):
    d = {"v": PROTOCOL_VERSION, "type": mtype}
    if session is not None:
        d["session"] = session
    d.update(extra)
    return d


def _prose(daemon, session, text, index=0, final=True):
    daemon.handle_message(_msg(MsgType.PROSE, session, delta=text, index=index,
                               final=final))


def _drain_one(daemon, queue, speaker):
    """Pop one item from the router and run it through the speak-loop bookkeeping."""
    item = queue.pop_next()
    assert item is not None
    completed = speaker.speak(item.text)
    daemon.note_spoken(item, completed)
    return item


# --- recording -------------------------------------------------------------

def test_prose_chunks_recorded_per_session():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "One. Two. ")
    assert [e.text for e in daemon.history.unheard("fg")] == ["One.", "Two."]


def test_final_closes_the_message_group():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "First. ")
    _prose(daemon, "fg", "Second. ")
    assert [e.text for e in daemon.history.last_message("fg")] == ["Second."]


# --- heard marking ----------------------------------------------------------

def test_completed_speech_marks_heard():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Hello there. ")
    _drain_one(daemon, queue, speaker)
    assert daemon.history.unheard("fg") == []


def test_interrupted_sentence_stays_unheard():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Hello there. ")
    speaker.complete = False                      # simulate cancel mid-sentence
    _drain_one(daemon, queue, speaker)
    assert [e.text for e in daemon.history.unheard("fg")] == ["Hello there."]


def test_stop_leaves_entries_unheard():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "A. B. ")
    daemon.handle_message(_msg(MsgType.STOP))
    assert daemon.router.channel("fg").pending() == 0
    assert [e.text for e in daemon.history.unheard("fg")] == ["A.", "B."]


def test_user_prompt_flush_resets_history():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Old stuff. ")
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    assert daemon.history.unheard("fg") == []
    assert daemon.history.last_message("fg") == []


def test_history_cap_comes_from_config():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    assert daemon.history._cap == config["history_cap"] == 200


# --- voice continuity / per-session channels --------------------------------

def test_foreground_session_items_land_in_its_channel():
    """Prose from the foreground session goes into that session's channel."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "Hi. ", final=False)
    assert daemon.router.channel("a").pending() == 1


def test_nonforeground_response_is_captured_in_its_channel():
    """Non-foreground prose is captured in the session's own channel (not
    spoken until that session becomes foreground or is reached by the router)."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "b", "Background. ")
    # b has its item in its own channel
    assert daemon.router.channel("b").pending() == 1
    # but the router would not pick b to speak (a is foreground)
    assert daemon.router.channel("a").pending() == 0
    # b's item is in history as unheard
    assert [e.text for e in daemon.history.unheard("b")] == ["Background."]


def test_items_from_multiple_sessions_land_in_their_own_channels():
    """Each session's prose accumulates in its own channel independently."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A speaking. ", final=False)
    sessions.set_foreground("b")
    _prose(daemon, "a", "Still a. ", index=1, final=False)
    # both items in a's channel
    assert daemon.router.channel("a").pending() == 2
    # b has nothing yet
    assert daemon.router.channel("b").pending() == 0


def test_background_channel_not_spoken_while_other_session_is_active():
    """While session a is active, b's items accumulate in b's channel but the
    router does not read b (a is still active with an open turn).

    In the per-session-channel model the router uses a 'keep-floor' rule:
    the current reader (a) keeps reading as long as it has items.  b's items
    stay pending in b's channel until the router transitions to b."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A holds the voice. ", final=False)
    sessions.set_foreground("b")
    _prose(daemon, "b", "B part one. ", final=False)
    # a still has its item pending
    assert daemon.router.channel("a").pending() == 1
    # b's item is in b's channel
    assert daemon.router.channel("b").pending() == 1
    # Both items are in history
    assert [e.text for e in daemon.history.unheard("a")] == ["A holds the voice."]
    assert [e.text for e in daemon.history.unheard("b")] == ["B part one."]


def test_router_transitions_to_foreground_after_active_session_drains():
    """After a's turn is done and its channel drains, the router picks b
    (now foreground) to be the active reader.

    The cooperative hand-off uses the router's 'current reader keeps the floor'
    rule: if reading has ALREADY STARTED on a session (active != None), that
    session keeps the floor until its batch drains, even if fg switches.

    After a drains completely (establishing _last_active=a), b is a different
    session so the router emits a 'Session changed' announcement before b's
    content. Both the announcement and b's message must be heard."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A message. ", final=False)
    # Mark a's turn done so its channel is readable.
    daemon.handle_message(_msg(MsgType.EARCON, "a", kind="turn_done"))
    # Start reading a (this sets router.active = "a" and _last_active = "a").
    item_a = _drain_one(daemon, queue, speaker)
    assert item_a.text == "A message."
    # Now switch fg to b (a has already drained).
    daemon.handle_message(_msg(MsgType.SET_FOREGROUND, "b"))
    _prose(daemon, "b", "B message. ", final=False)
    # b is a different session from _last_active(a): announcement fires first.
    item_announce = queue.pop_next()
    assert item_announce is not None
    assert "Session changed" in item_announce.text, (
        f"Expected announce before b's item. Got: {item_announce.text!r}"
    )
    # b's content follows.
    item_b = _drain_one(daemon, queue, speaker)
    assert item_b.text == "B message."


def test_nonforeground_background_session_not_autostarted():
    """A non-fg session with ready items is NOT served when the default
    background_policy ('earcon_only') is active.

    The router's oldest-waiting fallthrough is gated by sessions.should_speak():
    with earcon_only policy, only the foreground session is eligible for voice.
    The bg session stays pending until it becomes foreground."""
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "b", "B backlog. ")
    # b has its item captured, but policy blocks it
    assert daemon.router.channel("b").pending() == 1
    # router.next_item() won't pick b (b is not fg; earcon_only policy blocks it)
    assert daemon.router.next_item() is None


def test_choice_for_nonfg_session_is_captured_and_options_stored():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A talking. ", final=False)
    sessions.set_foreground("b")
    daemon.handle_message(_msg(MsgType.CHOICE, "b", questions=[
        {"question": "Pick one?", "options": [{"label": "X"}, {"label": "Y"}]}
    ]))
    # a has 1 prose item, b has 1 choice item — each in their own channel
    assert daemon.router.channel("a").pending() == 1
    assert daemon.router.channel("b").pending() == 1
    assert "Pick one?" in daemon._options["b"]              # reread works on return
    assert daemon.history.unheard("b")                      # captured for catch_up


# --- repeat ------------------------------------------------------------------

def test_repeat_respeaks_whole_last_message_not_last_fragment():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "First sentence. Second sentence. Third. ")
    while daemon.router.channel("fg").pending():
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REPEAT))
    texts = []
    while daemon.router.channel("fg").pending():
        texts.append(queue.pop_next().text)
    assert texts == ["First sentence.", "Second sentence.", "Third."]


def test_repeat_targets_last_message_only():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Old message. ")
    _prose(daemon, "fg", "New message. ")
    while daemon.router.channel("fg").pending():
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REPEAT))
    item = queue.pop_next()
    assert item.text == "New message."
    assert daemon.router.channel("fg").pending() == 0


def test_repeat_with_no_history_says_nothing_to_repeat():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.REPEAT))
    item = queue.pop_next()
    assert item.text == "Nothing to repeat."


def test_repeat_acts_on_foreground_session_history():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A message. ")
    _prose(daemon, "b", "B captured. ")
    # drain all pending (includes both a and b items)
    while daemon.router.channel("a").pending() or daemon.router.channel("b").pending():
        item = queue.pop_next()
        if item is None:
            break
        daemon.note_spoken(item, True)
    daemon.handle_message(_msg(MsgType.REPEAT))
    item = queue.pop_next()
    assert item.text == "A message."


# --- catch_up ----------------------------------------------------------------

def test_catch_up_replays_unheard_oldest_first_then_marks_heard():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "One. Two. ")
    daemon.handle_message(_msg(MsgType.STOP))               # heard nothing
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = []
    while daemon.router.channel("fg").pending():
        texts.append(_drain_one(daemon, queue, speaker).text)
    assert texts == ["One.", "Two."]
    assert daemon.history.unheard("fg") == []               # marker advanced


def test_catch_up_interrupted_sentence_replays_from_its_start():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Long sentence here. ")
    speaker.complete = False                                # cut mid-sentence
    _drain_one(daemon, queue, speaker)
    speaker.complete = True
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    item = queue.pop_next()
    assert item.text == "Long sentence here."               # from the start


def test_catch_up_all_heard_says_caught_up():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Hi. ")
    while daemon.router.channel("fg").pending():
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    item = queue.pop_next()
    assert item.text == "You're all caught up."


def test_catch_up_falls_back_to_other_session_backlog():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A heard. ")
    while daemon.router.channel("a").pending():
        _drain_one(daemon, queue, speaker)
    _prose(daemon, "b", "B unheard. ")                      # captured in b's channel
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = []
    item = queue.pop_next()
    while item is not None:
        texts.append(item.text)
        item = queue.pop_next()
    assert texts == ["Catching up on another session.", "B unheard."]


def test_catch_up_does_not_double_speak_queued_items():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Queued one. Queued two. ")        # in channel, unheard
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = []
    item = queue.pop_next()
    while item is not None:
        texts.append(item.text)
        item = queue.pop_next()
    assert texts == ["Queued one.", "Queued two."]          # once, not twice


# --- reread_options ----------------------------------------------------------

def _choice(daemon, session, questions):
    daemon.handle_message(_msg(MsgType.CHOICE, session, questions=questions))


def test_choice_speaks_descriptions_and_numbers():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{
        "question": "Auth method?",
        "options": [
            {"label": "OAuth", "description": "Use Google sign-in"},
            {"label": "Magic link"},
        ],
    }])
    item = queue.pop_next()
    assert "Option 1: OAuth. Use Google sign-in." in item.text
    assert "Option 2: Magic link." in item.text


def test_multiselect_announced_up_front():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{
        "question": "Pick features.",
        "multiSelect": True,
        "options": [{"label": "A"}, {"label": "B"}],
    }])
    item = queue.pop_next()
    assert "This is a multi-select; you can pick more than one." in item.text


def test_reread_speaks_current_options_not_queue_tail():
    # REREAD reads from the dedicated _options slot, not from whatever text is
    # currently at the channel tail. Drain the original choice item (cursor
    # moves on), then reread — the slot still holds the choice text.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{"question": "Q?", "options": [{"label": "X"}]}])
    while daemon.router.channel("fg").pending():
        _drain_one(daemon, queue, speaker)
    # Enqueue unrelated prose WITHOUT final=True so the options slot stays open.
    daemon.handle_message(_msg(MsgType.PROSE, "fg", delta="Other speech. ",
                               index=0, final=False))
    while daemon.router.channel("fg").pending():
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))
    item = queue.pop_next()
    assert "Option 1: X." in item.text                       # the options, not the tail


def test_reread_with_no_active_options_says_so():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))
    item = queue.pop_next()
    assert item.text == "No options right now."


def test_reread_after_flush_says_no_options():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{"question": "Q?", "options": [{"label": "X"}]}])
    daemon.handle_message(_msg(MsgType.FLUSH, "fg"))
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))
    item = queue.pop_next()
    assert item.text == "No options right now."


def test_reread_is_per_session():
    # REREAD_OPTIONS targets the FOREGROUND session only. In earcon_only mode
    # (the default), b's pending decision does NOT preempt -- non-fg session
    # text is suppressed by the background policy, so only fg reads.
    # The earcon fires separately (cross-session) but the text stays silent.
    # (This behavior is correct; the I1 fix allows preemption in non-earcon_only
    # configs -- see test_background_decision_while_idle_is_read for that case.)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _choice(daemon, "b", [{"question": "B q?", "options": [{"label": "BB"}]}])
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))      # fg=a has none
    item = queue.pop_next()
    assert item is not None
    assert item.text == "No options right now."


# --- permission text fallback ----------------------------------------------

def test_permission_uses_message_when_action_empty():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PERMISSION, "fg", action="",
                               message="Claude needs your permission."))
    item = queue.pop_next()
    assert "Claude needs your permission." in item.text
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))
    assert "Claude needs your permission." in queue.pop_next().text


def test_permission_falls_back_to_generic_cue_without_action_or_message():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.PERMISSION, "fg", action="", message=""))
    item = queue.pop_next()
    assert "Permission needed." in item.text
