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

    def fake(session, gen, text, token=0, leadin=False, seq=None):
        calls.append((session, gen, text, token))
        # Free the ordering slot (#88): these tests run workers manually with
        # seq=None (sequencer bypass), which would otherwise leave the slot
        # parked and block later synchronous lands (background short turns).
        daemon._land_digest(seq, None)

    monkeypatch.setattr(daemon, "_start_summary_thread", fake)
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
    session, gen, text = calls[0][:3]
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
    # is DIGESTED (#83: raw was mostly "let me check the repo" process noise)
    # and its recap is heard BEFORE the question once the digest lands.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Here is the short context. "))
    _choice(daemon)
    assert len(calls) == 1                     # short lead-in digested, not raw (#83)
    daemon._summarize_fn = lambda text, **kw: "The short context, recapped."
    daemon._summary_worker(*calls[0], leadin=True)
    ch = daemon.router.channel("fg")
    items = ch.items[ch.cursor:]
    prose_idx = next(i for i, it in enumerate(items) if "recapped" in it.text)
    dec_idx = next(i for i, it in enumerate(items) if it.is_decision)
    assert prose_idx < dec_idx                 # context read before the question
    # the RAW narration never reached the channel
    assert not any("Here is the short context" in it.text for it in items)


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


def test_short_lead_in_question_held_with_capped_release(monkeypatch):
    # (#83) short lead-ins are digested now, so the question holds - but the
    # hold is CAPPED: the release timer speaks it even if the digest stalls.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    timers = []
    monkeypatch.setattr(daemon, "_schedule_hold_release",
                        lambda s, o, i: timers.append((s, o, i)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Short context. "))
    _choice(daemon)
    assert daemon._held_decision.get("fg") is not None
    assert len(timers) == 1                     # cap armed alongside the hold
    daemon._release_held_decision(*timers[0])   # digest stalls -> cap fires
    ch = daemon.router.channel("fg")
    assert any(it.is_decision for it in ch.items[ch.cursor:])
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


def test_new_prompt_resets_voiced_marker(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._voiced_upto["fg"] = object()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH, "session": "fg"})
    assert "fg" not in daemon._voiced_upto


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
    _, _, text2 = calls[1][:3]
    assert "The recap." not in text2                     # recap (summary kind) excluded
    assert "New content." in text2                       # new prose IS summarized


def test_session_end_clears_await_choice(monkeypatch):
    # A session ending with an unanswered AskUserQuestion must not leave a stale
    # _await_choice entry: the permission-chime suppression check is GLOBAL
    # truthiness, so one stale entry would swallow every future permission chime
    # daemon-wide (audit #19).
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._await_choice.add("fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END,
                           "session": "fg"})
    assert "fg" not in daemon._await_choice


def test_short_turn_does_not_suppress_session_announcement(monkeypatch):
    # The short-turn path reused _replay, which pre-sets router._last_active to
    # suppress the "Session changed" announcement -- correct for user replays
    # (catch_up/nav/repeat), WRONG for automatic turn delivery: a short turn
    # after another session read played unattributed (audit #21).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    sessions.register("fg", cwd="/home/me/alpha")
    daemon.router.active = "other"
    daemon.router._last_active = "other"        # reader last read ANOTHER session
    daemon.handle_message(_prose("fg", "Short reply. ", 0, True))
    _turn_done(daemon)                          # short turn -> raw replay path
    assert daemon.router._last_active == "other"   # announce NOT pre-suppressed
    seq = []
    for _ in range(4):
        it = daemon.router.next_item()
        if it is None:
            break
        seq.append((it.kind, it.text))
    ann = next(i for i, (k, t) in enumerate(seq) if k == "session_change")
    txt = next(i for i, (k, t) in enumerate(seq) if "Short reply." in t)
    assert ann < txt                             # handoff announced before content


def test_reread_preserves_queued_question(monkeypatch):
    # Summary-mode Up (re-read) while a question sat queued DELETED the question
    # forever -- never re-inserted, nothing replays it (audit #21). Decision items
    # must survive the re-read and play after the digest.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _choice(daemon)                                      # digest in flight, question held
    daemon._summarize_fn = lambda text, **kw: "The context."
    daemon._summary_worker(*calls[0])                    # digest + question enqueued
    ch = daemon.router.channel("fg")
    assert any(it.is_decision for it in ch.items[ch.cursor:])
    assert daemon._reread_last("fg") is True             # Up during the digest read
    items = ch.items[ch.cursor:]
    assert any(it.is_decision for it in items)           # question PRESERVED
    kinds = [it.kind for it in items]
    assert kinds.index("summary") < kinds.index("choice")  # digest first, question after
    assert ch.has_decision                               # router still preempts for it


def test_plan_defers_through_settle_for_late_lead_in(monkeypatch):
    # PLAN raced its lead-in prose exactly like CHOICE (#16): the decision
    # message beat the MessageDisplay prose, the lead-in gather found nothing,
    # and the context was spoken late or dropped. PLAN now defers through the
    # settle window too (audit #21).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PLAN,
                           "session": "fg", "text": "Build the thing."})
    _pad = "This filler sentence carries the lead-in past the threshold. "
    daemon.handle_message(_prose("fg", "Plan lead-in. " + _pad * 6, 0, True))  # late
    daemon._settle_fire("fg", scheduled[-1][1])
    assert len(calls) == 1
    assert "Plan lead-in." in calls[0][2]                 # context digested, not lost
    assert daemon._held_decision.get("fg") is not None     # plan held behind it


def test_permission_defers_through_settle_for_late_lead_in(monkeypatch):
    # Same race, PERMISSION path (audit #21).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    scheduled = []
    monkeypatch.setattr(daemon, "_settle_schedule",
                        lambda session, gen: scheduled.append((session, gen)))
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PERMISSION,
                           "session": "fg", "action": "Run the migration?"})
    _pad = "This filler sentence carries the lead-in past the threshold. "
    daemon.handle_message(_prose("fg", "Permission lead-in. " + _pad * 6, 0, True))
    daemon._settle_fire("fg", scheduled[-1][1])
    assert len(calls) == 1
    assert "Permission lead-in." in calls[0][2]
    assert daemon._held_decision.get("fg") is not None


def test_turn_end_digest_survives_history_eviction(monkeypatch):
    # The voiced-prose position was an absolute COUNT into a capped deque: after
    # a mid-turn digest plus eviction on a long turn, later digests over-skipped
    # and silently dropped unvoiced prose (worst case: the whole turn-end digest,
    # violating 'never skip the last message') (audit #21). Track by identity.
    from sonara.history import SessionHistory
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.history = SessionHistory(cap=8)               # small cap to force eviction
    calls = _capture_spawn(daemon, monkeypatch)
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    pad = "This filler sentence carries the turn well past the threshold. "
    daemon.handle_message(_prose("fg", "Sentence one. " + pad * 3, 0, True))
    daemon.handle_message(_prose("fg", "Sentence two. " + pad * 3, 1, True))
    _turn_done(daemon)                                   # digest 1 voices both
    assert len(calls) == 1
    words = ["three", "four", "five", "six", "seven", "eight", "nine", "ten"]
    for i, w in enumerate(words):                        # 8 more -> evicts one+two
        daemon.handle_message(_prose("fg", "Sentence {0}. ".format(w) + pad * 3,
                                     2 + i, True))
    _turn_done(daemon)                                   # digest 2 must still fire
    # Old code: voiced-count 8 >= the 8 surviving entries -> gathered nothing ->
    # NO dispatch, the turn's final content silently skipped. With identity
    # tracking, the evicted marker means every surviving entry is unvoiced.
    assert len(calls) == 2
    text2 = calls[1][2]
    assert "Sentence nine." in text2 and "Sentence ten." in text2
    # Two same-gen digests in flight: the first to land must NOT pop the question
    # held for the second (its actual lead-in), or the question plays before its
    # own context (audit #21). Only the OWNING worker appends it.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # worker A (turn-end digest)
    _pad = "This filler sentence carries the turn well past the threshold. "
    daemon.handle_message(_prose("fg", "Question lead-in. " + _pad * 6, 2, True))
    _choice(daemon)                                      # worker B + question held
    assert len(calls) == 2
    daemon._summarize_fn = lambda text, **kw: "Digest A."
    daemon._summary_worker(*calls[0])                    # A lands first
    ch = daemon.router.channel("fg")
    assert not any(it.is_decision for it in ch.items)    # A did NOT take the question
    daemon._summarize_fn = lambda text, **kw: "Digest B."
    daemon._summary_worker(*calls[1])                    # owner lands
    pairs = [(it.kind, it.text) for it in ch.items]
    b_idx = next(i for i, (k, t) in enumerate(pairs) if t == "Digest B.")
    q_idx = next(i for i, (k, t) in enumerate(pairs) if k == "choice")
    assert b_idx < q_idx                                 # question AFTER its lead-in


def test_question_holds_behind_inflight_turn_digest(monkeypatch):
    # A question whose OWN lead-in gather finds nothing new must still hold while
    # an earlier digest of the same turn is in flight -- previously it was
    # enqueued immediately and played BEFORE its context (audit #21).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # digest in flight (all prose)
    _choice(daemon)                                      # no new prose since dispatch
    ch = daemon.router.channel("fg")
    assert not any(it.is_decision for it in ch.items[ch.cursor:])   # held, not enqueued
    daemon._summarize_fn = lambda text, **kw: "The context."
    daemon._summary_worker(*calls[0])                    # in-flight digest lands
    kinds = [it.kind for it in ch.items[ch.cursor:]]
    assert kinds.index("summary") < kinds.index("choice")  # context, then question


def test_session_end_cancels_inflight_digest(monkeypatch):
    # A digest in flight when its session ends must not be applied. Popping
    # _summary_gen reset never-FLUSHed sessions to a PASSING guard (get()==0 ==
    # dispatched gen 0), so the dead session's digest resurrected its history and
    # channel. SESSION_END now bumps the epoch like FLUSH (audit #21).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                    # dispatched with gen 0
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END,
                           "session": "fg"})
    daemon._summarize_fn = lambda text, **kw: "Ghost digest."
    daemon._summary_worker(*calls[0])                     # lands after the session died
    assert "fg" not in daemon.router.channels             # channel NOT resurrected
    assert not daemon.history.unheard("fg")               # history NOT resurrected


def test_session_end_clears_held_decision(monkeypatch):
    # SESSION_END never cleared _held_decision, so a late worker's finally block
    # appended the dead session's blocking question to a freshly recreated
    # channel (zombie question) even when the gen check failed (audit #21).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _choice(daemon)                                       # question held behind digest
    assert daemon._held_decision.get("fg") is not None
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END,
                           "session": "fg"})
    assert daemon._held_decision.get("fg") is None
    daemon._summarize_fn = lambda text, **kw: "Ghost."
    daemon._summary_worker(*calls[0])                     # late worker
    assert "fg" not in daemon.router.channels             # no zombie channel/question


def test_session_end_clears_per_session_state(monkeypatch):
    # The hand-copied FLUSH/SESSION_END cleanup lists drifted (audit #21): these
    # per-session containers leaked after SESSION_END.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon._last_digest_text["fg"] = "stale"
    daemon._voiced_upto["fg"] = object()
    daemon._nav_cursor["fg"] = 7
    daemon._assemblers["fg"] = object()
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.SESSION_END,
                           "session": "fg"})
    assert "fg" not in daemon._last_digest_text
    assert "fg" not in daemon._voiced_upto
    assert "fg" not in daemon._nav_cursor
    assert "fg" not in daemon._assemblers


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
    _, _, text = calls[0][:3]
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
    # (#83) the settle fire digests the lead-in (even short) and HOLDS the
    # question behind it; the digest landing releases it.
    assert daemon._held_decision.get("fg") is not None
    daemon._summarize_fn = lambda text, **kw: "ctx"
    daemon._summary_worker(*calls[-1], leadin=True)
    assert any(it.is_decision for it in ch.items[ch.cursor:])       # released


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


def test_reread_after_bare_question_replays_the_question(monkeypatch):
    # A turn that is ONLY an AskUserQuestion (no prose) left NOTHING re-readable:
    # summary-mode Up gave the edge chime and the user could never hear the
    # question again (live report, 2026-07-14). A HEARD question must append to
    # the re-read record.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    _choice(daemon)                                      # bare question, no prose
    for _ in range(4):
        daemon._speak_loop_once()                        # question is spoken
    assert any("Pick one?" in t for t in speaker.spoken)
    speaker.spoken.clear()
    assert daemon._reread_last("fg") is True             # was: False -> edge chime
    for _ in range(4):
        daemon._speak_loop_once()
    assert any("Pick one?" in t for t in speaker.spoken)  # question heard again


def test_reread_after_digest_and_question_replays_both(monkeypatch):
    # With a lead-in digest + question both heard, Up must re-read the WHOLE
    # unit (digest then question), not just the prose.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The digest."
    daemon._summary_worker(*calls[0])                    # digest lands
    _choice(daemon)
    for _ in range(6):
        daemon._speak_loop_once()                        # digest + question spoken
    assert any("Pick one?" in t for t in speaker.spoken)
    speaker.spoken.clear()
    assert daemon._reread_last("fg") is True
    for _ in range(6):
        daemon._speak_loop_once()
    joined = " ".join(speaker.spoken)
    assert "The digest." in joined and "Pick one?" in joined


def test_up_during_speaking_question_restarts_it(monkeypatch):
    # Up pressed WHILE the question is being spoken used to ANNIHILATE it: the
    # re-read cancelled the utterance, but a mid-speech question is not in the
    # re-read record yet (it joins on completion) and nothing re-queued it --
    # edge chime, question gone forever (live report, 2026-07-14). Up during a
    # speaking question must RESTART it.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    _choice(daemon)                                      # bare question, no prose
    with daemon._lock:                                   # loop takes the item...
        item = daemon.router.next_item()
        while item is not None and not item.is_decision:
            item = daemon.router.next_item()             # skip the mode-change cue
        daemon._current_item = item
    assert item is not None and item.is_decision
    # ...and MID-SPEECH the user presses Up:
    assert daemon._reread_last("fg") is True             # was: False -> edge chime
    daemon.note_spoken(item, False)                      # cancelled speak returns
    for _ in range(4):
        daemon._speak_loop_once()
    assert any("Pick one?" in t for t in speaker.spoken)  # question re-asked
    ch = daemon.router.channel("fg")
    qs = [it for it in ch.items if it.is_decision and "Pick one?" in it.text]
    assert len(qs) == 1                                  # restarted, not duplicated


def test_up_during_speaking_digest_does_not_double_speak(monkeypatch):
    # Regression guard: Up mid-DIGEST already restarts correctly by re-reading
    # the record; the interrupted digest item must not ALSO be re-queued.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = lambda text, **kw: "The digest."
    daemon._summary_worker(*calls[0])                    # digest queued + recorded
    with daemon._lock:
        item = daemon.router.next_item()                 # digest mid-speech
        daemon._current_item = item
    assert daemon._reread_last("fg") is True
    daemon.note_spoken(item, False)
    for _ in range(6):
        daemon._speak_loop_once()
    assert speaker.spoken.count("The digest.") == 1      # spoken once, not twice


def test_reread_does_not_double_append_on_repeat_rereads(monkeypatch):
    # The re-read insert is is_decision=False, so hearing a re-read must NOT
    # re-append the question to the record (unbounded growth).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    _choice(daemon)
    for _ in range(4):
        daemon._speak_loop_once()
    baseline = daemon._last_digest_text.get("fg")
    daemon._reread_last("fg")
    for _ in range(4):
        daemon._speak_loop_once()                        # re-read heard
    assert daemon._last_digest_text.get("fg") == baseline


def test_digest_text_is_normalized_for_speech(monkeypatch):
    # (#27) digests bypass the assembler's markdown cleaner, so underscores,
    # backticks, and arrows reached the TTS raw and were mispronounced.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)
    daemon._summarize_fn = (
        lambda text, **kw: "Renamed `get_user_id` -> `fetch_id` & re-ran.")
    daemon._summary_worker(*calls[0])
    ch = daemon.router.channel("fg")
    summ = next(it for it in ch.items[ch.cursor:] if it.kind == "summary")
    assert "_" not in summ.text and "`" not in summ.text
    assert "->" not in summ.text and "&" not in summ.text
    assert "get user id" in summ.text


def test_digest_dispatch_prewarms_chatterbox(monkeypatch):
    # (#27) the GPU model loads WHILE haiku digests, hiding the ~40s post-idle
    # cold reload inside the digest latency instead of stalling speech after it.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    warms = []
    monkeypatch.setattr(daemon, "_warm_chatterbox_async",
                        lambda: warms.append(True))
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # async digest dispatched
    assert warms                                         # warm kicked at dispatch


def test_short_foreground_turn_sets_reread_text(monkeypatch):
    # Deep audit (#25): short FOREGROUND turns never set _last_digest_text, so
    # summary-mode Up (the only hotkey re-read) gave a dead edge chime after a
    # short turn -- while short BACKGROUND turns set it and re-read fine.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    _set_mode(daemon, True)
    daemon.handle_message(_prose("fg", "Back on. ", 0, True))
    _turn_done(daemon)                                   # short turn, raw replay
    assert daemon._last_digest_text.get("fg") == "Back on."
    assert daemon._reread_last("fg") is True             # Up now works


def test_flush_clears_inflight_accounting_so_new_question_not_held(monkeypatch):
    # Deep audit (#25): FLUSH left _inflight_digests/_last_dispatch_token stale,
    # so a NEW turn's blocking question (short lead-in -> no own digest) was held
    # behind the FLUSH-cancelled digest and stayed SILENT until the stale worker
    # landed -- up to summary_timeout in an eyes-free tool.
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # digest W1 in flight
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH,
                           "session": "fg"})             # new prompt cancels W1
    daemon.handle_message(_prose("fg", "Short context. ", 0, True))
    _choice(daemon)                                      # must NOT wait for dead W1
    # (#83) the question holds behind its OWN fresh lead-in digest - never the
    # FLUSH-cancelled W1. Its owner token is the post-flush dispatch, and its
    # own digest landing releases it immediately.
    held = daemon._held_decision.get("fg")
    assert held is not None
    assert held[0] == daemon._last_dispatch_token["fg"]  # owned by W2, not dead W1
    assert len(calls) == 2                               # W1 + the new lead-in digest
    daemon._summarize_fn = lambda text, **kw: "ctx"
    daemon._summary_worker(*calls[-1], leadin=True)      # own digest lands
    ch = daemon.router.channel("fg")
    assert any(it.is_decision for it in ch.items[ch.cursor:])   # enqueued NOW


def test_stale_worker_does_not_steal_postflush_inflight_count(monkeypatch):
    # Companion (#25): after FLUSH cleared the count, the CANCELLED worker's
    # finally must not decrement a POST-flush dispatch's count (that would let a
    # later question jump ahead of its own in-flight context digest).
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    calls = _capture_spawn(daemon, monkeypatch)
    _enable_and_feed(daemon, monkeypatch)
    _turn_done(daemon)                                   # W1 (gen 0)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH,
                           "session": "fg"})             # gen -> 1; count cleared
    _pad = "This filler sentence carries the turn well past the threshold. "
    daemon.handle_message(_prose("fg", "New turn text. " + _pad * 6, 0, True))
    _turn_done(daemon)                                   # W2 (gen 1), count = 1
    daemon._summarize_fn = lambda text, **kw: "Stale W1."
    daemon._summary_worker(*calls[0])                    # stale W1 lands
    assert daemon._inflight_digests.get("fg", 0) == 1    # W2 still counted


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
    _, _, text = calls[0][:3]
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
