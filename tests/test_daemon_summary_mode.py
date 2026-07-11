"""Summary mode: SET_SUMMARY_MODE toggles + prose is recorded but not spoken."""
from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _prose(session, text, idx=0, final=True):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": session,
            "delta": text, "index": idx, "final": final}


def _set_mode(daemon, enabled):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_SUMMARY_MODE,
                           "enabled": enabled})


def test_set_summary_mode_toggles_and_persists(monkeypatch):
    import sonara.daemon as daemon_module
    saved = []
    monkeypatch.setattr(daemon_module, "save_config", saved.append)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    assert config["summary_mode"] is True
    _set_mode(daemon, False)
    assert config["summary_mode"] is False
    assert len(saved) == 2


def test_set_summary_mode_without_enabled_is_noop(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SET_SUMMARY_MODE})
    assert config["summary_mode"] is False


def test_summary_mode_suppresses_prose_speech_but_records_history(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "A long explanation. "))
    ch = daemon.router.channel("fg")
    assert ch.pending() == 0                              # nothing queued to speak
    assert daemon.history.unheard("fg")                   # but history recorded it


def test_summary_mode_off_prose_is_spoken_as_today():
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.handle_message(_prose("fg", "Hello there. "))
    assert daemon.router.channel("fg").pending() > 0


def test_decisions_still_spoken_with_summary_mode_on(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE,
                           "session": "fg",
                           "questions": [{"question": "Pick one?",
                                          "options": ["a", "b"]}]})
    ch = daemon.router.channel("fg")
    assert any(it.is_decision for it in ch.items[ch.cursor:])


def test_status_reports_summary_mode(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    reply = daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.STATUS})
    assert reply["summary_mode"] is True
