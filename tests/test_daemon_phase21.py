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
