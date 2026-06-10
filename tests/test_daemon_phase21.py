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
