"""#83: question-flow fixes - digested lead-ins, bounded hold, answer catch-up.

User reports: (1) short lead-ins before questions read RAW process narration
("let me check the repo"); (2) answering a question did nothing - the stale
backlog and late lead-in digests kept speaking; (3) questions sat silent for
the summarizer's whole 10-40s call (context-first hold was unbounded).
"""
from tests.daemon_helpers import make_daemon
from sonara.protocol import MsgType, PROTOCOL_VERSION
from sonara.queue import SpeechItem


def _summary_daemon(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="fg")
    daemon.config["summary_mode"] = True
    spawned = []

    def fake(session, gen, text, token=0, leadin=False, seq=None):
        spawned.append({"session": session, "gen": gen, "text": text,
                        "token": token, "leadin": leadin})
        daemon._land_digest(seq, None)   # free the ordering slot (#88)

    monkeypatch.setattr(daemon, "_start_summary_thread", fake)
    return daemon, speaker, spawned


def _prose(daemon, text, session="fg", idx=0):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.PROSE,
                           "session": session, "delta": text, "index": idx,
                           "final": True})


def _question(daemon, session="fg"):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE,
                           "session": session,
                           "questions": [{"question": "Deploy now?",
                                          "options": [{"label": "Yes"}]}]})


def _fire_settle(daemon, session="fg"):
    gen = daemon._settle_gen.get(session)
    daemon._settle_fire(session, gen)


# --- A: lead-ins before questions are digested, never raw --------------------

def test_short_leadin_before_question_is_digested_not_raw(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    _prose(daemon, "Let me check out this repo. ")     # 28 chars, way under 280
    _question(daemon)
    _fire_settle(daemon)
    assert len(spawned) == 1                            # async digest dispatched
    assert spawned[0]["leadin"] is True
    assert "check out this repo" in spawned[0]["text"]
    # nothing raw got enqueued on the channel (the old behavior replayed it)
    ch = daemon.router.channel("fg")
    texts = [i.text for i in ch.items]
    assert not any("check out this repo" in t for t in texts)
    # the question is HELD behind the digest, not enqueued yet
    assert "fg" in daemon._held_decision


def test_short_turn_end_without_question_still_replays_raw(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    _prose(daemon, "All done here. ")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    _fire_settle(daemon)
    assert spawned == []                                # no model call
    ch = daemon.router.channel("fg")
    assert any("All done here." in i.text for i in ch.items)


def test_leadin_skip_digest_drops_silently_and_releases_question(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    daemon._summarize_fn = lambda text, **kw: None      # SKIP/failed
    _prose(daemon, "Let me verify this. ")
    _question(daemon)
    _fire_settle(daemon)
    d = spawned[0]
    # run the real worker with the SKIP summarizer
    daemon._summary_worker(d["session"], d["gen"], d["text"], d["token"],
                           leadin=d["leadin"])
    ch = daemon.router.channel("fg")
    texts = [i.text for i in ch.items]
    assert not any("Let me verify" in t for t in texts)  # noise dropped, no raw fallback
    assert any(i.is_decision for i in ch.items)          # question still released
    assert "fg" not in daemon._held_decision


def test_turn_end_skip_digest_keeps_raw_fallback(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    daemon._summarize_fn = lambda text, **kw: None
    long_text = "This is substantive content the user must hear. " * 8
    _prose(daemon, long_text)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    _fire_settle(daemon)
    d = spawned[0]
    assert d["leadin"] is False
    daemon._summary_worker(d["session"], d["gen"], d["text"], d["token"],
                           leadin=d["leadin"])
    ch = daemon.router.channel("fg")
    assert any("substantive content" in i.text for i in ch.items)  # raw fallback


# --- C: the question hold is capped ------------------------------------------

def test_hold_release_speaks_question_when_digest_is_slow(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    timers = []
    monkeypatch.setattr(daemon, "_schedule_hold_release",
                        lambda s, o, i: timers.append((s, o, i)))
    _prose(daemon, "Let me check something first before asking. ")
    _question(daemon)
    _fire_settle(daemon)
    assert "fg" in daemon._held_decision
    assert len(timers) == 1
    daemon._release_held_decision(*timers[0])           # the 5s cap fires
    ch = daemon.router.channel("fg")
    assert any(i.is_decision for i in ch.items)          # question speaks NOW
    assert "fg" not in daemon._held_decision
    # the digest worker landing later must not re-release anything
    d = spawned[0]
    daemon._summarize_fn = lambda text, **kw: "The digest."
    daemon._summary_worker(d["session"], d["gen"], d["text"], d["token"],
                           leadin=d["leadin"])
    decisions = [i for i in ch.items if i.is_decision]
    assert len(decisions) == 1                           # not duplicated
    assert any("The digest." in i.text for i in ch.items)  # digest follows


def test_hold_release_is_noop_after_digest_landed(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    timers = []
    monkeypatch.setattr(daemon, "_schedule_hold_release",
                        lambda s, o, i: timers.append((s, o, i)))
    _prose(daemon, "Some context before the question arrives here. ")
    _question(daemon)
    _fire_settle(daemon)
    d = spawned[0]
    daemon._summarize_fn = lambda text, **kw: "The digest."
    daemon._summary_worker(d["session"], d["gen"], d["text"], d["token"],
                           leadin=d["leadin"])           # digest lands first
    ch = daemon.router.channel("fg")
    before = len(ch.items)
    daemon._release_held_decision(*timers[0])            # late timer fire
    assert len(ch.items) == before                       # idempotent no-op


# --- B: answering the question catches the user up ---------------------------

def test_choice_answered_flushes_backlog_and_cuts_current(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    ch = daemon.router.channel("fg")
    ch.append(SpeechItem(id=daemon._alloc_id(), session="fg", kind="prose",
                         text="stale backlog item", is_decision=False))
    daemon._current_item = SpeechItem(id=daemon._alloc_id(), session="fg",
                                      kind="summary", text="being spoken",
                                      is_decision=False)
    daemon.handle_message({"v": PROTOCOL_VERSION,
                           "type": MsgType.CHOICE_ANSWERED, "session": "fg"})
    assert ch.cursor == len(ch.items)                    # backlog skipped
    assert speaker.cancels == 1                          # current utterance cut
    assert daemon._current_item is None


def test_choice_answered_kills_inflight_leadin_digest(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    _prose(daemon, "A long enough lead-in before the question. " * 10)
    _question(daemon)
    _fire_settle(daemon)
    d = spawned[0]
    assert daemon._inflight_digests.get("fg")            # digest in flight
    daemon.handle_message({"v": PROTOCOL_VERSION,
                           "type": MsgType.CHOICE_ANSWERED, "session": "fg"})
    assert "fg" not in daemon._held_decision             # held question dropped
    assert not daemon._inflight_digests.get("fg")
    # the worker lands AFTER the answer: its gen is stale -> nothing speaks
    daemon._summarize_fn = lambda text, **kw: "Too late digest."
    daemon._summary_worker(d["session"], d["gen"], d["text"], d["token"],
                           leadin=d["leadin"])
    ch = daemon.router.channel("fg")
    texts = [i.text for i in ch.items[ch.cursor:]]
    assert not any("Too late digest." in t for t in texts)


def test_choice_answered_drops_pending_settle_decision(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    _prose(daemon, "Some lead-in. ")
    _question(daemon)                                    # deferred via settle
    assert "fg" in daemon._pending_decision
    daemon.handle_message({"v": PROTOCOL_VERSION,
                           "type": MsgType.CHOICE_ANSWERED, "session": "fg"})
    assert "fg" not in daemon._pending_decision
    assert "fg" not in daemon._await_choice


def test_flush_session_gets_the_same_summary_semantics(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    _prose(daemon, "A long enough lead-in before the question. " * 10)
    _question(daemon)
    _fire_settle(daemon)
    assert daemon._inflight_digests.get("fg")
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.FLUSH_SESSION})
    assert "fg" not in daemon._held_decision
    assert not daemon._inflight_digests.get("fg")
    assert "nav" in speaker.earcons                      # acknowledged with a chime


def test_post_answer_prose_still_flows(monkeypatch):
    daemon, speaker, spawned = _summary_daemon(monkeypatch)
    _prose(daemon, "Lead-in before question. ")
    _question(daemon)
    daemon.handle_message({"v": PROTOCOL_VERSION,
                           "type": MsgType.CHOICE_ANSWERED, "session": "fg"})
    # the assistant continues after the answer; the turn-end digest covers it
    _prose(daemon, "Here is what I did after your answer. " * 10, idx=0)
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": "fg"})
    _fire_settle(daemon)
    assert spawned, "post-answer turn-end digest must still dispatch"
    assert "after your answer" in spawned[-1]["text"]
    assert "Lead-in before question" not in spawned[-1]["text"]
