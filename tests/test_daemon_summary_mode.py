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


# --- turn-end summary dispatch ------------------------------------------

def _turn_done(daemon, session="fg"):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": session})


def _capture_spawn(daemon, monkeypatch):
    calls = []
    monkeypatch.setattr(daemon, "_start_summary_thread",
                        lambda session, gen, text: calls.append((session, gen, text)))
    return calls


def _enable_and_feed(daemon, monkeypatch, session="fg"):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    # Pad both parts past the short-turn threshold (_SUMMARY_MIN_CHARS), so
    # these fixtures exercise the summarizer dispatch, not the pass-through.
    pad = "This filler sentence carries the turn well past the threshold. "
    daemon.handle_message(_prose(session, "First part. " + pad * 3, 0, True))
    daemon.handle_message(_prose(session, "Second part. " + pad * 3, 1, True))


def test_turn_done_dispatches_summary_for_foreground(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    assert len(calls) == 1
    session, gen, text = calls[0]
    assert session == "fg"
    assert "First part." in text and "Second part." in text


def test_turn_done_does_not_dispatch_when_mode_off(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    daemon.handle_message(_prose("fg", "Text. "))
    _turn_done(daemon)
    assert calls == []


def test_background_turn_dispatches_summary_too(monkeypatch):
    # Multi-session: a background session's finished turn is summarized as
    # well (its digest speaks prefixed with the session folder). Foreground-
    # only gating silently dropped digests whenever the user prompted another
    # session mid-turn (observed live).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch, session="bg")
    _turn_done(daemon, session="bg")
    assert len(calls) == 1 and calls[0][0] == "bg"


def test_background_digest_speaks_prefixed_via_control(monkeypatch):
    from sonara.router import CONTROL
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    sessions.register("bg", cwd="/home/me/otherproj")
    _enable_and_feed(daemon, monkeypatch, session="bg")
    _turn_done(daemon, session="bg")
    daemon._summarize_fn = lambda text, **kw: "The gist of it."
    daemon._summary_worker(*calls[0])
    ctrl = daemon.router.channel(CONTROL)
    texts = [it.text for it in ctrl.items[ctrl.cursor:]]
    assert "Session otherproj: The gist of it." in texts
    # and nothing landed in the bg session's own (unvoiced) channel
    bg = daemon.router.channel("bg")
    assert all("gist" not in it.text for it in bg.items[bg.cursor:])


def test_background_short_turn_speaks_original_prefixed(monkeypatch):
    from sonara.router import CONTROL
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    sessions.register("bg", cwd="/home/me/otherproj")
    daemon.handle_message(_prose("bg", "All done here. ", 0, True))
    _turn_done(daemon, session="bg")
    assert calls == []                                    # no summarizer for short
    ctrl = daemon.router.channel(CONTROL)
    texts = [it.text for it in ctrl.items[ctrl.cursor:]]
    assert any(t.startswith("Session otherproj: ") and "All done here." in t
               for t in texts)


def test_turn_done_with_no_prose_does_not_dispatch(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _set_mode(daemon, True)
    _turn_done(daemon)                                   # decision-only / empty turn
    assert calls == []


def test_worker_success_enqueues_summary(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The gist."
    daemon._summary_worker(*calls[0])                    # run inline, outside the lock
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert "The gist." in texts


def test_worker_failure_fires_cue_and_enqueues_nothing(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: None
    daemon._summary_worker(*calls[0])
    assert speaker.earcons[-1] == "summary_failed"
    assert daemon.router.channel("fg").pending() == 0


def test_superseded_worker_result_is_dropped(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # gen 1
    daemon.handle_message(_prose("fg", "More text. ", 2, True))
    _turn_done(daemon)                                   # gen 2 supersedes
    daemon._summarize_fn = lambda text, **kw: "Stale summary."
    daemon._summary_worker(*calls[0])                    # gen-1 result arrives late
    ch = daemon.router.channel("fg")
    assert "Stale summary." not in [it.text for it in ch.items[ch.cursor:]]


def test_worker_forwards_config_to_summarizer(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    config["summary_model"] = "haiku"
    config["summary_command"] = "claude"
    config["summary_timeout"] = 20
    _turn_done(daemon)
    seen = {}

    def fake(text, **kw):
        seen.update(kw)
        return "Recap."
    daemon._summarize_fn = fake
    daemon._summary_worker(*calls[0])
    assert seen["model"] == "haiku" and seen["command"] == "claude"
    assert seen["timeout"] == 20                      # explicit config wins
    assert callable(seen["debug_log"])                # failure-reason sink wired


# --- FLUSH supersedes in-flight summary; recap kind excludes re-gather --

def test_flush_supersedes_inflight_summary(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # gen 1 in flight
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH,
                           "session": "fg"})             # new prompt
    daemon._summarize_fn = lambda text, **kw: "Stale recap."
    daemon._summary_worker(*calls[0])                    # late gen-1 result
    ch = daemon.router.channel("fg")
    assert "Stale recap." not in [it.text for it in ch.items[ch.cursor:]]
    assert not daemon.history.unheard("fg")              # nothing recorded either


def test_recorded_summary_is_not_resummarized(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The recap."
    daemon._summary_worker(*calls[0])                    # recap recorded
    _turn_done(daemon)                                   # second turn_done, no flush
    assert len(calls) == 2
    _, _, text2 = calls[1]
    assert "The recap." not in text2                     # recap excluded from gather


def test_session_end_clears_summary_gen(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._summary_gen["fg"] = 3
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END,
                           "session": "fg"})
    assert "fg" not in daemon._summary_gen


# --- short turns skip the summarizer and speak the original text ----------

def test_short_turn_speaks_original_without_summarizer(monkeypatch):
    # Digesting an already-short message adds nothing and risks the model
    # emitting spoken meta-text on borderline "trivial" input (observed live).
    # Below the threshold the daemon replays the original prose directly.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Back on. ", 0, True))
    _turn_done(daemon)
    assert calls == []                                    # no summarizer spawned
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert any("Back on." in t for t in texts)            # original spoken instead


def test_long_turn_still_dispatches_summarizer(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    long_text = "This sentence pads the turn well past the threshold. " * 12
    daemon.handle_message(_prose("fg", long_text, 0, True))
    _turn_done(daemon)
    assert len(calls) == 1
