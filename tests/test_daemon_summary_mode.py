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
    _fire_settle(daemon, "fg")                   # question content settles then enqueues (#16)
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
    _fire_settle(daemon, session)


def _fire_settle(daemon, session="fg"):
    # Deterministic settle: cancel the real timer and fire synchronously, so
    # turn-end tests do not wait on the clock (#14).
    gen = daemon._settle_gen.get(session)
    if gen is None:
        return
    t = daemon._settle_timers.pop(session, None)
    if t is not None:
        t.cancel()
    daemon._settle_fire(session, gen)


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
    _fire_settle(daemon, session)   # question content is deferred through the settle (#16)


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


def test_held_question_context_for_other_session_uses_session_channel(monkeypatch):
    # Multi-session: a question from a NON-foreground session is a real handoff, so
    # its context digest must go via THAT session's channel (which announces the
    # "Session changed" switch BEFORE the context), not CONTROL (a silent
    # interjection that would leave the announcement to fire at the question).
    from sonara.router import CONTROL
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    sessions.register("bg", cwd="/home/me/sonari")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch, session="bg")   # long prose to bg
    _choice(daemon, session="bg")                          # holds bg's question
    daemon._summarize_fn = lambda text, **kw: "The context."
    daemon._summary_worker(*calls[0])
    bg = daemon.router.channel("bg")
    ctrl = daemon.router.channel(CONTROL)
    assert any("The context." in it.text for it in bg.items)          # digest on bg channel
    assert not any("The context." in it.text for it in ctrl.items)    # NOT via CONTROL
    assert any(it.is_decision for it in bg.items)                     # question follows it


def test_held_question_falls_back_to_raw_context_on_skip(monkeypatch):
    # Confirmed via sequence capture: Haiku SKIP'd a trivial lead-in (summarize ->
    # None), so the held question played with no context. A held question must fall
    # back to the RAW lead-in, spoken before the question, so it is never alone.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)               # "First part. ... Second part. ..."
    _choice(daemon)                                      # holds the question
    daemon._summarize_fn = lambda text, **kw: None       # SKIP / empty digest
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    items = ch.items[ch.cursor:]
    summ_idx = next(i for i, it in enumerate(items) if it.kind == "summary")
    dec_idx = next(i for i, it in enumerate(items) if it.is_decision)
    assert summ_idx < dec_idx                            # raw context before the question
    assert "First part." in items[summ_idx].text         # the raw lead-in, not dropped


def test_turn_end_skip_falls_back_to_raw(monkeypatch):
    # A session's LATEST message is always read (spec): if the summarizer SKIPs a
    # plain turn-end digest, fall back to the RAW text rather than dropping it.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)                # "First part. ... Second part. ..."
    _turn_done(daemon)                                   # no held question
    daemon._summarize_fn = lambda text, **kw: None       # SKIP
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    summ = next((it for it in ch.items[ch.cursor:] if it.kind == "summary"), None)
    assert summ is not None and "First part." in summ.text   # raw, not dropped


def test_empty_turn_stays_silent(monkeypatch):
    # A genuinely empty turn (no text at all) still stays silent -- nothing to read.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.config["summary_mode"] = True
    # dispatch a worker directly with empty text
    daemon._summarize_fn = lambda text, **kw: None
    daemon._summary_gen["fg"] = 1
    daemon._summary_worker("fg", 1, "   ")
    ch = daemon.router.channel("fg")
    assert not any(it.kind == "summary" for it in ch.items[ch.cursor:])


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


def test_held_question_lead_in_digest_is_unprefixed(monkeypatch):
    # The context digest is never prefixed with "Session X:" -- the router's
    # "Session changed" announcement is the sole session identifier (#15).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    sessions.register("fg", cwd="/home/me/scenario")     # folder = scenario
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _choice(daemon)                                       # holds the question
    daemon._summarize_fn = lambda text, **kw: "The context."
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    summ = next(it for it in ch.items[ch.cursor:] if it.kind == "summary")
    assert summ.text == "The context."


def test_normal_turn_end_digest_is_unprefixed(monkeypatch):
    # A plain turn-end digest is never prefixed with "Session X:" (#15); the
    # "Session changed" announcement names the session on a switch instead.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    sessions.register("fg", cwd="/home/me/scenario")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The recap."
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    summ = next(it for it in ch.items[ch.cursor:] if it.kind == "summary")
    assert summ.text == "The recap."                      # no prefix


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


def test_background_digest_announced_via_session_channel(monkeypatch):
    # A background session's digest now goes via ITS OWN channel so the router
    # announces "Session changed" (with the chime) before speaking it -- not the
    # silent CONTROL lane that played out of order, chime-less (multi-session bug).
    # It is unprefixed (the announcement names the session) and authorized so the
    # background policy does not mute it.
    from sonara.router import CONTROL
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    sessions.register("bg", cwd="/home/me/otherproj")
    _enable_and_feed(daemon, monkeypatch, session="bg")
    _turn_done(daemon, session="bg")
    daemon._summarize_fn = lambda text, **kw: "The gist of it."
    daemon._summary_worker(*calls[0])
    bg = daemon.router.channel("bg")
    ctrl = daemon.router.channel(CONTROL)
    assert any(it.text == "The gist of it." for it in bg.items)   # on bg channel, unprefixed
    assert not any("gist" in it.text for it in ctrl.items)        # NOT on CONTROL
    assert "bg" in daemon.router._replay_authorized               # policy-bypassed for voicing


def test_background_digest_announced_before_it_plays(monkeypatch):
    # End-to-end: the router announces "Session changed: otherproj" (a chime-
    # carrying session_change item) BEFORE the background digest is spoken.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    sessions.register("bg", cwd="/home/me/otherproj")
    daemon.router.active = "fg"
    daemon.router._last_active = "fg"                     # a switch to bg will announce
    _enable_and_feed(daemon, monkeypatch, session="bg")
    _turn_done(daemon, session="bg")
    daemon._summarize_fn = lambda text, **kw: "The gist of it."
    daemon._summary_worker(*calls[0])
    seq = []
    for _ in range(6):
        it = daemon.router.next_item()
        if it is None:
            break
        seq.append((it.kind, it.text))
    ann = next(i for i, (k, t) in enumerate(seq) if k == "session_change" and "otherproj" in t)
    dig = next(i for i, (k, t) in enumerate(seq) if t == "The gist of it.")
    assert ann < dig                                     # announcement precedes the digest


def test_background_short_turn_announced_via_session_channel(monkeypatch):
    # A short background turn now goes via its OWN channel (announced with a chime,
    # unprefixed, authorized), not the silent CONTROL lane that played out of order
    # and survived FLUSH.
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
    bg = daemon.router.channel("bg")
    ctrl = daemon.router.channel(CONTROL)
    assert any("All done here." in it.text for it in bg.items)        # on bg channel
    assert not any("All done here." in it.text for it in ctrl.items)  # NOT on CONTROL
    assert "bg" in daemon.router._replay_authorized


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


def test_worker_failure_falls_back_to_raw(monkeypatch):
    # A summarizer failure (returns None) now falls back to the RAW text -- the
    # session's latest message is always read (spec), never silently dropped.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: None
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    summ = next((it for it in ch.items[ch.cursor:] if it.kind == "summary"), None)
    assert summ is not None and "First part." in summ.text   # read raw, not dropped


def test_second_turn_end_keeps_first_digest(monkeypatch):
    # User-only cancel (#13): two turn-ends on ONE session with NO user action
    # between them must BOTH be read. A turn merely ending no longer drops the
    # prior turn's finished digest (was: the second turn superseded the first).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # first digest dispatched
    _pad = "This filler sentence carries the turn past the threshold. "
    daemon.handle_message(_prose("fg", "More text. " + _pad * 6, 2, True))
    _turn_done(daemon)                                   # second digest dispatched
    daemon._summarize_fn = lambda text, **kw: "First digest."
    daemon._summary_worker(*calls[0])                    # first result lands late
    ch = daemon.router.channel("fg")
    assert "First digest." in [it.text for it in ch.items[ch.cursor:]]  # NOT dropped


def test_both_queued_digests_play_without_user_action(monkeypatch):
    # "Read all queued" (#13): every finished digest of a session is enqueued when
    # no user action intervenes -- none is superseded by a later turn ending.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # digest 1 dispatched
    _pad = "This filler sentence carries the turn past the threshold. "
    daemon.handle_message(_prose("fg", "Second turn. " + _pad * 6, 2, True))
    _turn_done(daemon)                                   # digest 2 dispatched
    assert len(calls) == 2
    results = iter(["Digest one.", "Digest two."])
    daemon._summarize_fn = lambda text, **kw: next(results)
    daemon._summary_worker(*calls[0])
    daemon._summary_worker(*calls[1])
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert "Digest one." in texts and "Digest two." in texts


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


def test_foreground_digest_is_unprefixed(monkeypatch):
    # A foreground digest is never prefixed (#15): the user is already in that
    # session, and a switch would announce "Session changed" on its own.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    sessions.set_foreground("fg", cwd="/home/me/myrepo")
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The gist."
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    texts = [it.text for it in ch.items[ch.cursor:]]
    assert "The gist." in texts
    assert not any(t.startswith("Session myrepo:") for t in texts)   # no prefix


# --- question lead-in settle (#16) ---------------------------------------

def test_question_lead_in_after_choice_not_stranded(monkeypatch):
    # The bug: CHOICE arrives before its lead-in prose. With the settle window the
    # late lead-in is digested and the question is held after it (#16).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE, "session": "fg",
                           "questions": [{"question": "Pick one?", "options": ["a", "b"]}]})
    _pad = "This filler sentence carries the lead-in past the threshold. "
    daemon.handle_message(_prose("fg", "The context here. " + _pad * 6, 0, True))  # late lead-in
    daemon._settle_fire("fg", scheduled[-1][1])
    assert len(calls) == 1
    _, _, text = calls[0]
    assert "The context here." in text                     # lead-in digested, not empty
    assert daemon._held_decision.get("fg") is not None      # question held until digest


def test_choice_defers_question_until_settle(monkeypatch):
    # CHOICE alone does not enqueue the question; only the settle fire does (#16).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Short context. ", 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE, "session": "fg",
                           "questions": [{"question": "Pick one?", "options": ["a", "b"]}]})
    ch = daemon.router.channel("fg")
    assert not any(it.is_decision for it in ch.items[ch.cursor:])   # deferred
    daemon._settle_fire("fg", scheduled[-1][1])
    assert any(it.is_decision for it in ch.items[ch.cursor:])       # enqueued after fire


def test_flush_cancels_pending_question_settle(monkeypatch):
    # A new prompt during a question's settle window drops the pending question;
    # a late fire is a no-op (#16, consistent with #13/#14).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE, "session": "fg",
                           "questions": [{"question": "Pick one?", "options": ["a", "b"]}]})
    stale = scheduled[-1][1]
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    daemon._settle_fire("fg", stale)
    ch = daemon.router.channel("fg")
    assert not any(it.is_decision for it in ch.items[ch.cursor:])   # dropped
    assert "fg" not in daemon._pending_decision


# --- queued question is not overtaken by a later short turn (#17) ---------

def test_short_answer_does_not_overtake_held_question(monkeypatch):
    # Repro of the live bug (#17): answer a question before it is voiced; the short
    # answer-response must NOT cut ahead of the queued question in the channel.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)           # long lead-in
    _choice(daemon)                                  # settle -> digest dispatched, question held
    daemon._summarize_fn = lambda text, **kw: "The context digest."
    daemon._summary_worker(*calls[0])                # context enqueued + question appended
    ch = daemon.router.channel("fg")
    daemon.handle_message(_prose("fg", "Short answer. ", 0, True))   # short answer-response
    _turn_done(daemon)                               # settle -> short path
    texts = [it.text for it in ch.items]
    q_idx = next(i for i, t in enumerate(texts) if "Pick one?" in t)
    a_idx = next(i for i, t in enumerate(texts) if "Short answer." in t)
    assert q_idx < a_idx                             # question stays ahead of the later short answer


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


# --- turn-end settle window (#14) ----------------------------------------

def test_turn_done_defers_digest_until_settle(monkeypatch):
    # turn_done alone must NOT dispatch: the turn's final prose may still be in
    # flight (separate hook processes race). Only the settle fire dispatches (#14).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    _enable_and_feed(daemon, monkeypatch)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    assert calls == []                       # deferred: no digest yet
    assert scheduled and scheduled[-1][0] == "fg"
    daemon._settle_fire("fg", scheduled[-1][1])
    assert len(calls) == 1                    # settle fired -> dispatched


def test_late_prose_included_after_turn_done(monkeypatch):
    # The bug: turn_done arrives before the paragraph body. With the settle
    # window, the late body is included and the turn takes the digest path (#14).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Here is another one:", 0, True))   # short lead-in
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})           # arms window
    _pad = "This filler sentence carries the turn well past the threshold. "
    daemon.handle_message(_prose("fg", " " + _pad * 6, 1, True))            # late body -> re-arm
    daemon._settle_fire("fg", scheduled[-1][1])                            # window fires
    assert len(calls) == 1
    _, _, text = calls[0]
    assert "Here is another one:" in text and "filler sentence" in text     # FULL turn digested


def test_settle_window_resets_on_new_prose(monkeypatch):
    # New prose after turn_done re-arms the window; a fire from the superseded
    # (earlier) window is a no-op, only the latest fire dispatches (#14).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    _enable_and_feed(daemon, monkeypatch)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    first_gen = scheduled[-1][1]
    _pad = "This filler sentence carries the turn well past the threshold. "
    daemon.handle_message(_prose("fg", " " + _pad * 6, 2, True))            # re-arms
    second_gen = scheduled[-1][1]
    assert second_gen != first_gen
    daemon._settle_fire("fg", first_gen)                                    # stale
    assert calls == []
    daemon._settle_fire("fg", second_gen)                                  # current
    assert len(calls) == 1


def test_flush_cancels_pending_settle(monkeypatch):
    # A new prompt during the settle window abandons the turn: a late fire from
    # the cancelled window dispatches nothing (#14, consistent with #13).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    _enable_and_feed(daemon, monkeypatch)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    stale_gen = scheduled[-1][1]
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    daemon._settle_fire("fg", stale_gen)
    assert calls == []
    assert "fg" not in daemon._settle_pending
