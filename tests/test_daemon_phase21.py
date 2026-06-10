from sonari.protocol import MsgType, PROTOCOL_VERSION
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
    """Pop one queued item and run it through the speak-loop bookkeeping."""
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
    assert len(queue) == 0
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


# --- voice continuity / capture ---------------------------------------------

def test_foreground_session_acquires_free_voice():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "Hi. ", final=False)
    assert daemon._voice_owner == "a"
    assert len(queue) == 1


def test_nonforeground_response_is_captured_not_spoken():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "b", "Background. ")
    assert len(queue) == 0                                  # not spoken live
    assert [e.text for e in daemon.history.unheard("b")] == ["Background."]


def test_owner_keeps_voice_after_foreground_moves():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A speaking. ", final=False)        # a owns the voice
    sessions.set_foreground("b")                            # user prompts in b
    _prose(daemon, "a", "Still a. ", index=1, final=False)  # a keeps talking
    assert daemon._voice_owner == "a"
    assert len(queue) == 2


def test_response_landing_on_busy_voice_stays_captured_to_its_end():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A holds the voice. ", final=False)
    sessions.set_foreground("b")
    _prose(daemon, "b", "B part one. ", final=False)        # voice busy -> captured
    # a drains; voice frees
    while len(queue):
        _drain_one(daemon, queue, speaker)
    # b's SAME message continues -> still captured (no mid-thought join)
    _prose(daemon, "b", "B part two. ", index=1, final=True)
    assert len(queue) == 0
    texts = [e.text for e in daemon.history.unheard("b")]
    assert texts == ["B part one.", "B part two."]
    # b's NEXT message may acquire the free voice (b is foreground)
    _prose(daemon, "b", "B fresh message. ")
    assert daemon._voice_owner == "b"
    assert len(queue) == 1


def test_voice_frees_but_never_autostarts_nonforeground_backlog():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "b", "B backlog. ")                      # captured
    assert daemon._voice_owner is None
    assert len(queue) == 0                                  # stays silent


def test_choice_for_nonowner_is_captured_and_options_stored():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A talking. ", final=False)
    sessions.set_foreground("b")
    daemon.handle_message(_msg(MsgType.CHOICE, "b", questions=[
        {"question": "Pick one?", "options": [{"label": "X"}, {"label": "Y"}]}
    ]))
    assert len(queue) == 1                                  # only a's prose queued
    assert "Pick one?" in daemon._options["b"]              # reread works on return
    assert daemon.history.unheard("b")                      # captured for catch_up


# --- repeat ------------------------------------------------------------------

def test_repeat_respeaks_whole_last_message_not_last_fragment():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "First sentence. Second sentence. Third. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REPEAT))
    texts = []
    while len(queue):
        texts.append(queue.pop_next().text)
    assert texts == ["First sentence.", "Second sentence.", "Third."]


def test_repeat_targets_last_message_only():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Old message. ")
    _prose(daemon, "fg", "New message. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.REPEAT))
    item = queue.pop_next()
    assert item.text == "New message."
    assert len(queue) == 0


def test_repeat_with_no_history_says_nothing_to_repeat():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_msg(MsgType.REPEAT))
    item = queue.pop_next()
    assert item.text == "Nothing to repeat."


def test_repeat_acts_on_foreground_session_history():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A message. ")
    _prose(daemon, "b", "B captured. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
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
    while len(queue):
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
    while len(queue):
        _drain_one(daemon, queue, speaker)
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    item = queue.pop_next()
    assert item.text == "You're all caught up."


def test_catch_up_falls_back_to_other_session_backlog():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _prose(daemon, "a", "A heard. ")
    while len(queue):
        _drain_one(daemon, queue, speaker)
    _prose(daemon, "b", "B unheard. ")                      # captured silently
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = []
    while len(queue):
        texts.append(queue.pop_next().text)
    assert texts == ["Catching up on another session.", "B unheard."]


def test_catch_up_does_not_double_speak_queued_items():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _prose(daemon, "fg", "Queued one. Queued two. ")        # in queue, unheard
    daemon.handle_message(_msg(MsgType.CATCH_UP))
    texts = []
    while len(queue):
        texts.append(queue.pop_next().text)
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
    # currently at the queue tail. Drain the original choice item (queue tail
    # moves on), then reread — the slot still holds the choice text.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _choice(daemon, "fg", [{"question": "Q?", "options": [{"label": "X"}]}])
    while len(queue):
        _drain_one(daemon, queue, speaker)
    # Enqueue unrelated prose WITHOUT final=True so the options slot stays open.
    daemon.handle_message(_msg(MsgType.PROSE, "fg", delta="Other speech. ",
                               index=0, final=False))
    while len(queue):
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
    daemon, queue, speaker, sessions, config = make_daemon(foreground="a")
    _choice(daemon, "b", [{"question": "B q?", "options": [{"label": "BB"}]}])
    daemon.handle_message(_msg(MsgType.REREAD_OPTIONS))      # fg=a has none
    item = queue.pop_next()
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
