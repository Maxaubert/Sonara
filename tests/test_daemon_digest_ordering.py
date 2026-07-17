"""#88: multi-session digests must be HEARD in turn-finish (dispatch) order.

Digests used to become audible in summarizer-COMPLETION order (10-40s calls,
high variance), tie-broken by channel creation order and the reader floor -
with 3+ sessions finishing near-simultaneously the heard order was random and
stray late "Session X" digests surfaced arbitrarily.
"""
from sonara.protocol import MsgType, PROTOCOL_VERSION
from tests.daemon_helpers import make_daemon


def _prose(session, text, idx=0, final=True):
    return {"v": PROTOCOL_VERSION, "type": MsgType.PROSE, "session": session,
            "delta": text, "index": idx, "final": final}


def _turn_done(daemon, session):
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.EARCON,
                           "kind": "turn_done", "session": session})
    gen = daemon._settle_gen.get(session)
    t = daemon._settle_timers.pop(session, None)
    if t is not None:
        t.cancel()
    daemon._settle_fire(session, gen)


def _ordering_daemon(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon(foreground="user")
    import sonara.daemon as daemon_module
    monkeypatch.setattr(daemon_module, "save_config", lambda cfg: None)
    daemon.config["summary_mode"] = True
    calls = []
    monkeypatch.setattr(
        daemon, "_start_summary_thread",
        lambda session, gen, text, token=0, leadin=False, seq=None:
            calls.append({"session": session, "gen": gen, "text": text,
                          "token": token, "leadin": leadin, "seq": seq}))
    for s in ("user", "a", "b", "c"):
        sessions.register(s, cwd="/w/" + s)
    return daemon, speaker, calls


def _run_worker(daemon, d):
    daemon._summary_worker(d["session"], d["gen"], d["text"], d["token"],
                           leadin=d["leadin"], seq=d["seq"])


def _drain(daemon, speaker, n=30):
    daemon._poll_interval = 0.01
    for _ in range(n):
        daemon._speak_loop_once()
    return list(speaker.spoken)


_PAD = "This filler sentence carries the turn well past the threshold. "


def _finish_turn(daemon, session):
    daemon.handle_message(_prose(session, "Report from {0}. ".format(session)
                                 + _PAD * 6, 0, True))
    _turn_done(daemon, session)


def test_digests_heard_in_turn_finish_order_not_completion_order(monkeypatch):
    daemon, speaker, calls = _ordering_daemon(monkeypatch)
    # channels get created in order a, b, c ...
    for s in ("a", "b", "c"):
        daemon.handle_message(_prose(s, "warmup ", 0, False))
    # ... but the TURNS finish in order b, c, a
    for s in ("b", "c", "a"):
        _finish_turn(daemon, s)
    assert [d["session"] for d in calls] == ["b", "c", "a"]
    daemon._summarize_fn = (
        lambda text, **kw: "Recap " + text.split("Report from ")[1][0])
    # the summarizer completes in a THIRD order: a, c, b
    by = {d["session"]: d for d in calls}
    for s in ("a", "c", "b"):
        _run_worker(daemon, by[s])
    heard = [t for t in _drain(daemon, speaker) if t.startswith("Recap ")]
    assert heard == ["Recap b", "Recap c", "Recap a"]   # turn-finish order


def test_every_cross_session_digest_is_announced(monkeypatch):
    # the "missed chimes" half: each background digest is a real handoff and
    # must be preceded by its session-change announcement
    daemon, speaker, calls = _ordering_daemon(monkeypatch)
    # the user has been listening to their own session (a first-ever reader
    # legitimately never announces, so simulate prior reading)
    daemon.router._last_active = "user"
    for s in ("b", "c", "a"):
        _finish_turn(daemon, s)
    daemon._summarize_fn = (
        lambda text, **kw: "Recap " + text.split("Report from ")[1][0])
    by = {d["session"]: d for d in calls}
    for s in ("c", "a", "b"):
        _run_worker(daemon, by[s])
    heard = _drain(daemon, speaker)
    # #94: the announcement is now deferred to the content's own on_play, so it
    # is spoken via speak_cue_untracked (never enters `heard`) instead of
    # appearing in `heard` ahead of its recap. Each recap must still have been
    # preceded by its own handoff alert, in the same order.
    recaps = [t for t in heard if t.startswith("Recap ")]
    alerts = [text for text, _voice in speaker.cue_untracked_calls]
    assert len(recaps) == len(alerts) == 3
    for recap, alert in zip(recaps, alerts):
        session = recap.rsplit(" ", 1)[-1]
        assert session in alert, (session, alert, alerts)  # announcement named it first


def test_cancelled_digest_releases_the_ordering_slot(monkeypatch):
    # b's digest is killed (user answered / prompted); c's must not wait forever
    daemon, speaker, calls = _ordering_daemon(monkeypatch)
    for s in ("b", "c"):
        _finish_turn(daemon, s)
    by = {d["session"]: d for d in calls}
    daemon._summarize_fn = (
        lambda text, **kw: "Recap " + text.split("Report from ")[1][0])
    _run_worker(daemon, by["c"])                       # c completes FIRST, parks
    heard = [t for t in _drain(daemon, speaker, n=6) if t.startswith("Recap ")]
    assert heard == []                                 # parked behind b
    daemon._summary_gen["b"] = daemon._summary_gen.get("b", 0) + 1  # b cancelled
    _run_worker(daemon, by["b"])                       # lands dead, frees the slot
    heard = [t for t in _drain(daemon, speaker) if t.startswith("Recap ")]
    assert heard == ["Recap c"]                        # c released, b never speaks


def test_short_background_turn_joins_the_sequence(monkeypatch):
    # a SHORT background turn (no model call) finishing AFTER a long one must
    # not jump ahead of the long turn's still-cooking digest
    daemon, speaker, calls = _ordering_daemon(monkeypatch)
    _finish_turn(daemon, "b")                          # long: digest dispatched
    daemon.handle_message(_prose("c", "Quick note from c. ", 0, True))
    _turn_done(daemon, "c")                            # short: synchronous path
    heard = [t for t in _drain(daemon, speaker, n=6) if "Quick note" in t]
    assert heard == []                                 # parked behind b's digest
    daemon._summarize_fn = (
        lambda text, **kw: "Recap " + text.split("Report from ")[1][0])
    by = {d["session"]: d for d in calls}
    _run_worker(daemon, by["b"])
    heard = _drain(daemon, speaker)
    b_at = heard.index("Recap b")
    c_at = next(i for i, t in enumerate(heard) if "Quick note" in t)
    assert b_at < c_at                                 # long-first order preserved


def test_leadin_digests_bypass_the_sequencer(monkeypatch):
    # a question's lead-in digest is latency-critical (#83): it must not park
    # behind another session's slow turn-end digest
    daemon, speaker, calls = _ordering_daemon(monkeypatch)
    _finish_turn(daemon, "b")                          # slow digest in flight
    daemon.handle_message(_prose("user", "Context before question. " * 15, 0, True))
    daemon.handle_message({"v": PROTOCOL_VERSION, "type": MsgType.CHOICE,
                           "session": "user",
                           "questions": [{"question": "Go?", "options": ["y"]}]})
    gen = daemon._settle_gen.get("user")
    t = daemon._settle_timers.pop("user", None)
    if t is not None:
        t.cancel()
    daemon._settle_fire("user", gen)
    leadin = next(d for d in calls if d["leadin"])
    assert leadin["seq"] is None                       # bypasses the reorder buffer
    daemon._summarize_fn = lambda text, **kw: "Question context recap."
    _run_worker(daemon, leadin)
    heard = _drain(daemon, speaker)
    assert any("Question context recap." in t for t in heard)  # spoke despite b pending
