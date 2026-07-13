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


def test_foreground_digest_stores_exact_spoken_text_for_reread(monkeypatch):
    # The exact spoken digest text is stored so summary-mode Up re-reads it
    # verbatim -> the cached audio replays instead of regenerating (issue #11 f/u).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The digest body."
    daemon._summary_worker(*calls[0])
    assert daemon._last_digest_text.get("fg") == "The digest body."


def test_new_prompt_clears_stored_reread_text(monkeypatch):
    # A new prompt supersedes the previous turn: Up should not re-read a stale
    # digest, so the stored text is cleared on FLUSH.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._last_digest_text["fg"] = "old digest"
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    assert "fg" not in daemon._last_digest_text


def _choice(daemon, session="fg"):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE, "session": session,
                           "questions": [{"question": "Pick one?", "options": ["a", "b"]}]})


def test_blocking_question_voices_short_lead_in_prose(monkeypatch):
    # A question blocks the turn (no turn_done -> no digest). The short lead-in
    # prose must still be voiced (raw), BEFORE the question, not silently dropped.
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Here is the short context. "))
    _choice(daemon)
    ch = daemon.router.channel("fg")
    items = ch.items[ch.cursor:]
    prose_idx = next(i for i, it in enumerate(items) if "short context" in it.text)
    dec_idx = next(i for i, it in enumerate(items) if it.is_decision)
    assert prose_idx < dec_idx                 # context read before the question


def test_blocking_question_holds_until_long_lead_in_digest_lands(monkeypatch):
    # Long lead-in gets the AI digest dispatched, and the question is HELD (not
    # enqueued) until the digest lands, so context is heard BEFORE the question.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)      # long prose, past the threshold
    _choice(daemon)
    assert len(calls) == 1                      # digest dispatched for the lead-in
    ch = daemon.router.channel("fg")
    assert not any(it.is_decision for it in ch.items[ch.cursor:])  # question HELD
    assert daemon._held_decision.get("fg") is not None
    daemon._summarize_fn = lambda text, **kw: "The context recap."
    daemon._summary_worker(*calls[0])           # digest lands
    kinds = [it.kind for it in ch.items[ch.cursor:]]
    assert kinds.index("summary") < kinds.index("choice")   # context before question
    assert daemon._held_decision.get("fg") is None


def test_short_lead_in_question_not_held(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Short context. "))
    _choice(daemon)
    ch = daemon.router.channel("fg")
    assert any(it.is_decision for it in ch.items[ch.cursor:])   # short = synchronous
    assert daemon._held_decision.get("fg") is None


def test_no_lead_in_question_not_held(monkeypatch):
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    _set_mode(daemon, True)
    _choice(daemon)                             # no prose to voice
    ch = daemon.router.channel("fg")
    assert any(it.is_decision for it in ch.items[ch.cursor:])


def test_held_question_played_even_if_digest_fails(monkeypatch):
    # A blocking prompt must never be lost: if the digest fails, the held question
    # is still enqueued (finally).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _choice(daemon)
    daemon._summarize_fn = lambda text, **kw: None
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    assert any(it.is_decision for it in ch.items[ch.cursor:])   # question still played
    assert daemon._held_decision.get("fg") is None


def test_held_question_lead_in_digest_names_its_session(monkeypatch):
    # The context digest for a HELD question names its session too, so the user
    # hears which session the upcoming question belongs to.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    sessions.register("fg", cwd="/home/me/scenario")     # folder = scenario
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _choice(daemon)                                       # holds the question
    daemon._summarize_fn = lambda text, **kw: "The context."
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    summ = next(it for it in ch.items[ch.cursor:] if it.kind == "summary")
    assert summ.text == "Session scenario: The context."


def test_normal_turn_end_digest_keeps_session_prefix(monkeypatch):
    # A plain turn-end digest (no held question) still names its session.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    sessions.register("fg", cwd="/home/me/scenario")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The recap."
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    summ = next(it for it in ch.items[ch.cursor:] if it.kind == "summary")
    assert summ.text == "Session scenario: The recap."    # prefix kept


def test_new_prompt_clears_held_question(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _choice(daemon)
    assert daemon._held_decision.get("fg") is not None
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    assert daemon._held_decision.get("fg") is None


def test_lead_in_prose_not_double_voiced_at_turn_end(monkeypatch):
    # After the question voices the lead-in prose, a later turn_done (once the
    # user answers and the turn ends with no new prose) must NOT re-voice it.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _choice(daemon)
    assert len(calls) == 1
    _turn_done(daemon)                          # turn ends, no new prose
    assert len(calls) == 1                       # not re-dispatched


def test_new_prompt_resets_voiced_prose_count(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._voiced_prose_count["fg"] = 3
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    assert "fg" not in daemon._voiced_prose_count


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
    _pad = "This filler sentence carries the turn past the threshold. "
    daemon.handle_message(_prose("fg", "More text. " + _pad * 6, 2, True))
    _turn_done(daemon)                                   # gen 2 supersedes (new prose)
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
    daemon._summary_worker(*calls[0])                    # recap recorded (kind=summary)
    _pad = "This filler sentence carries the turn past the threshold. "
    daemon.handle_message(_prose("fg", "New content. " + _pad * 6, 2, True))
    _turn_done(daemon)                                   # second dispatch (new prose)
    assert len(calls) == 2
    _, _, text2 = calls[1]
    assert "The recap." not in text2                     # recap (summary kind) excluded
    assert "New content." in text2                       # new prose IS summarized


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


def test_foreground_digest_is_prefixed_with_its_folder(monkeypatch):
    # "Always announce the session speaking": every digest names its session,
    # foreground included (the user could not tell which session a digest
    # belonged to when sessions interleaved).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    sessions.set_foreground("fg", cwd="/home/me/myrepo")
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The gist."
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert "Session myrepo: The gist." in texts


def test_foreground_digest_without_folder_stays_unprefixed(monkeypatch):
    # No folder name -> nothing useful to announce; speak the digest bare
    # rather than "Session another session: ...".
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The gist."
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert "The gist." in texts
