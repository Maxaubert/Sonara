"""SPEAK (#143): an embedding host asks Sonara to read a text aloud.

The text goes to a session of its own, f"{source}:{tab or 'default'}",
registered with the host's label. Queue-of-one: a new SPEAK replaces that
session's unread text instead of queueing behind it, and it is spoken as
given (cleaned for speech, never summarized). interrupt=true also cuts that
session's current utterance, and only that session's."""
from __future__ import annotations

from sonara import client
from sonara.protocol import MsgType, PROTOCOL_VERSION
from sonara.queue import SpeechItem
from tests.daemon_helpers import make_daemon


def _speak(daemon, text, source="prism", tab="tab-1", label=None,
           interrupt=False):
    return daemon.handle_message({
        "v": PROTOCOL_VERSION, "type": MsgType.SPEAK, "text": text,
        "source": source, "tab": tab, "label": label, "interrupt": interrupt})


def _pending_texts(daemon, sid):
    ch = daemon.router.channels.get(sid)
    return [it.text for it in ch.pending_items()] if ch is not None else []


def test_speak_message_type_is_speak():
    assert MsgType.SPEAK == "speak"


def test_speak_registers_its_session_and_reads_the_text():
    daemon, queue, _sp, sessions, _cfg = make_daemon(foreground="fg")
    assert _speak(daemon, "Build **passed**.", label="Build tab") is None
    sid = "prism:tab-1"
    assert sid in sessions.ids()
    assert daemon.session_prefs.name(sid) == "Build tab"
    assert sessions.host_tab(sid) == "tab-1"
    # Not the foreground session, yet it is voiced: the host asked for it.
    item = queue.pop_next()
    assert (item.session, item.kind, item.text) == (sid, "summary", "Build passed.")


def test_speak_without_tab_uses_the_default_session():
    daemon, queue, _sp, sessions, _cfg = make_daemon(foreground="fg")
    _speak(daemon, "Hello.", tab=None)
    assert "prism:default" in sessions.ids()
    assert sessions.host_tab("prism:default") is None
    assert queue.pop_next().session == "prism:default"


def test_new_speak_replaces_the_unread_text_queue_of_one():
    daemon, _q, _sp, _s, _cfg = make_daemon(foreground="fg")
    _speak(daemon, "First.")
    _speak(daemon, "Second.")
    _speak(daemon, "Third.")
    assert _pending_texts(daemon, "prism:tab-1") == ["Third."]
    # The replaced items leave no heard-marker behind.
    pending_ids = {it.id for it in
                   daemon.router.channels["prism:tab-1"].pending_items()}
    assert set(daemon._pending_heard) == pending_ids


def test_speak_queues_are_per_tab():
    daemon, _q, _sp, _s, _cfg = make_daemon(foreground="fg")
    _speak(daemon, "One.", tab="a")
    _speak(daemon, "Two.", tab="b")
    assert _pending_texts(daemon, "prism:a") == ["One."]
    assert _pending_texts(daemon, "prism:b") == ["Two."]


def test_speak_is_never_summarized_in_summary_mode():
    daemon, queue, _sp, _s, cfg = make_daemon(foreground="fg")
    cfg["summary_mode"] = True
    _speak(daemon, "Read me as is.")
    assert queue.pop_next().text == "Read me as is."


def test_speak_interrupt_cuts_only_its_own_session():
    daemon, _q, speaker, _s, _cfg = make_daemon(foreground="fg")
    daemon._current_item = SpeechItem(id=99, session="fg", kind="prose",
                                      text="x", is_decision=False)
    _speak(daemon, "Now.", interrupt=True)
    assert speaker.cancels == 0                 # another session is speaking
    daemon._current_item = SpeechItem(id=98, session="prism:tab-1",
                                      kind="summary", text="old",
                                      is_decision=False)
    _speak(daemon, "Newer.", interrupt=False)
    assert speaker.cancels == 0                 # no interrupt asked
    _speak(daemon, "Newest.", interrupt=True)
    assert speaker.cancels == 1
    assert _pending_texts(daemon, "prism:tab-1") == ["Newest."]


def test_speak_keeps_the_global_pause():
    daemon, _q, _sp, _s, _cfg = make_daemon(foreground="fg")
    daemon._paused.set()
    _speak(daemon, "Held.")
    assert daemon._paused.is_set()


def test_speak_without_label_keeps_a_user_given_name():
    daemon, _q, _sp, _s, _cfg = make_daemon(foreground="fg")
    _speak(daemon, "A.", label="Host label")
    daemon.session_prefs.set("prism:tab-1", "name", "My name")
    _speak(daemon, "B.")
    assert daemon.session_prefs.name("prism:tab-1") == "My name"


def test_empty_speak_clears_the_unread_text():
    daemon, _q, _sp, _s, _cfg = make_daemon(foreground="fg")
    _speak(daemon, "Stale.")
    _speak(daemon, "  **  ")
    assert _pending_texts(daemon, "prism:tab-1") == []


def test_malformed_speak_is_ignored():
    daemon, _q, _sp, sessions, _cfg = make_daemon(foreground="fg")
    before = sessions.ids()
    _speak(daemon, None)
    _speak(daemon, "x", source="")
    _speak(daemon, "x", source=None)
    _speak(daemon, "x", tab=5)
    assert sessions.ids() == before
    assert all(not ch.pending() for ch in daemon.router.channels.values())


def test_speak_session_ends_like_any_other():
    daemon, _q, _sp, sessions, _cfg = make_daemon(foreground="fg")
    _speak(daemon, "Bye.")
    daemon.handle_message({"v": 1, "type": MsgType.SESSION_END,
                           "session": "prism:tab-1"})
    assert "prism:tab-1" not in sessions.ids()
    assert "prism:tab-1" not in daemon.router.channels
    assert daemon._pending_heard == {}


def test_speak_session_end_forgets_its_label():
    # Host tab ids change on every host run: a SPEAK session's persisted
    # name must not outlive SESSION_END, or labels pile up on disk.
    daemon, *_ = make_daemon(foreground="fg")
    _speak(daemon, "Bye.", label="Build tab")
    daemon.handle_message({"v": 1, "type": MsgType.SESSION_END,
                           "session": "prism:tab-1"})
    assert daemon.session_prefs.get("prism:tab-1") == {}


def test_claude_session_end_keeps_its_prefs():
    daemon, *_ = make_daemon(foreground="fg")
    daemon.session_prefs.set("abc-123", "name", "Mine")
    daemon.handle_message({"v": 1, "type": MsgType.SESSION_END,
                           "session": "abc-123"})
    assert daemon.session_prefs.name("abc-123") == "Mine"


def test_client_speak_sends_the_speak_message(monkeypatch):
    sent = []
    monkeypatch.setattr(client, "send",
                        lambda msg, expect_reply=False, timeout=2.0: sent.append(msg))
    client.speak("Hi.", "prism", "tab-1")
    client.speak("Yo.", "other", label="Other", interrupt=True)
    assert sent == [
        {"v": PROTOCOL_VERSION, "type": "speak", "text": "Hi.",
         "source": "prism", "tab": "tab-1", "label": None, "interrupt": False},
        {"v": PROTOCOL_VERSION, "type": "speak", "text": "Yo.",
         "source": "other", "tab": None, "label": "Other", "interrupt": True},
    ]
