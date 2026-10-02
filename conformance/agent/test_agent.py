"""Extension ``agent`` (spec 4.3): streamed text, turns, decisions, earcons
and mute levels, observed through state, item and earcon events. Parity
cases ported from the Python daemon tests that encode product rules: one
message always the last, late text after a new turn dropped (#174), the
pause stays on when another channel gets a new turn (upstream #69),
decisions spoken with priority, the question's permission prompt
suppressed (#11), mute levels (earcons stay at level 1)."""
from __future__ import annotations

import time

from harness import long_text

LONG = long_text(1)
TWO_LONG = long_text(2)


def agent_client(rt, announce=False, policy="earcon_only"):
    """A TCP client with the agent extension on, subscribed to state, items
    and earcons, past the first (idle) state. The parity cases run on the
    Python plugin's settings (prose read at once, every kind of text,
    ``policy`` as the background speech policy, default ``earcon_only``),
    not on the product defaults of #202 (five chunks held, ``medium``,
    ``all``)."""
    c = rt.tcp(extensions=["agent"])
    assert c.request({"type": "subscribe", "events": ["state", "items", "earcons"]})["ok"]
    c.state()
    ok(c, {"type": "set", "key": "channel_announce", "value": "on" if announce else "off"})
    ok(c, {"type": "set", "key": "minqueue", "value": 1})
    ok(c, {"type": "set", "key": "verbosity", "value": "everything"})
    ok(c, {"type": "set", "key": "background_policy", "value": policy})
    return c


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def stream(c, channel, delta, index=0, t=None, final=True):
    msg = {"type": "stream", "channel": channel, "delta": delta, "index": index, "final": final}
    if t is not None:
        msg["t"] = t
    return ok(c, msg)


def heard(c, count):
    """The next ``count`` items to start, as (first chunk text, channel)."""
    out, last = [], getattr(c, "last_item", None)
    while len(out) < count:
        e = c.next_event(lambda e: e.get("event") == "state")
        np = e["now_playing"]
        if np is None:
            continue
        if np["item_id"] != last:
            last = np["item_id"]
            out.append((np["text"], np.get("channel")))
    c.last_item = last
    return out


def earcon(c, timeout=5.0):
    return c.next_event(lambda e: e.get("event") == "earcon", timeout)["kind"]


def no_earcon(c, seconds=0.5):
    try:
        e = c.next_event(lambda e: e.get("event") == "earcon", seconds)
    except AssertionError:
        return
    raise AssertionError(f"unexpected earcon {e}")


def quiet(c, seconds=0.6):
    """Nothing starts for ``seconds``."""
    end = time.monotonic() + seconds
    while True:
        left = end - time.monotonic()
        if left <= 0:
            return
        try:
            e = c.next_event(lambda e: e.get("event") == "state", timeout=left)
        except AssertionError:
            return
        assert e["now_playing"] is None, e


def snapshot(rt):
    """The current state, from a fresh subscription."""
    probe = rt.tcp()
    try:
        assert probe.request({"type": "subscribe", "events": ["state"]})["ok"]
        return probe.state()
    finally:
        probe.close()


def starts_long(item, channel, n=0):
    text, ch = item
    return text.startswith(f"Sentence {n}") and ch == channel


def test_hello_enables_agent_with_channels(rt):
    assert "agent" in rt.read_runtime()["extensions"]
    plain = rt.tcp()
    for msg in (
        {"type": "stream", "channel": "a", "delta": "Hi."},
        {"type": "turn_start", "channel": "a"},
        {"type": "ask", "channel": "a", "kind": "plan"},
        {"type": "get", "key": "mute_level"},
    ):
        assert plain.request(msg)["error"]["code"] == "E_UNSUPPORTED", msg
    r = plain.request({"type": "subscribe", "events": ["earcons"]})
    assert r["error"]["code"] == "E_UNSUPPORTED"
    c = rt.tcp(hello=False)
    r = c.hello(rt.token, extensions=["agent"])
    assert r["extensions"] == ["channels", "agent"]
    assert r["unavailable"] == []
    r = ok(plain, {"type": "stream", "channel": "a", "delta": "Hi.", "final": True})
    assert r["stale"] is False


def test_a_turn_is_read_sentence_by_sentence(rt):
    c = agent_client(rt)
    ok(c, {"type": "turn_start", "channel": "a", "t": 1.0})
    stream(c, "a", "First sentence here. Second ", 0, final=False)
    stream(c, "a", "sentence here.", 1)
    assert heard(c, 2) == [("First sentence here.", "a"), ("Second sentence here.", "a")]
    quiet(c)


def test_one_message_always_the_last(rt):
    # A new turn cuts what is left of the previous one: its item and its
    # unread sentences; only the new turn is read.
    c = agent_client(rt)
    ok(c, {"type": "turn_start", "channel": "a", "t": 1.0})
    stream(c, "a", TWO_LONG, t=1.0)
    (first,) = heard(c, 1)
    assert starts_long(first, "a")
    ok(c, {"type": "turn_start", "channel": "a", "t": 2.0})
    stream(c, "a", "The new answer.", t=2.0)
    assert heard(c, 1) == [("The new answer.", "a")]
    quiet(c)


def test_late_text_after_a_new_turn_is_dropped(rt):
    # #174: hook events are separate processes, so the old turn's last text
    # can arrive after the new prompt. Stamped with its sender's start time,
    # it is dropped.
    c = agent_client(rt)
    ok(c, {"type": "turn_start", "channel": "a", "t": 105.0})
    assert stream(c, "a", "Old tail.", 3, t=104.0)["stale"] is True
    r = ok(c, {"type": "turn_end", "channel": "a", "t": 104.5})
    assert r["stale"] is True
    no_earcon(c)
    quiet(c, 0.3)
    assert stream(c, "a", "New text.", 0, t=106.0)["stale"] is False
    assert heard(c, 1) == [("New text.", "a")]


def test_text_of_an_earlier_turn_id_is_dropped(rt):
    c = agent_client(rt)
    ok(c, {"type": "turn_start", "channel": "a", "turn": "t1"})
    ok(c, {"type": "turn_start", "channel": "a", "turn": "t2"})
    r = ok(c, {"type": "stream", "channel": "a", "turn": "t1", "delta": "Late.", "final": True})
    assert r["stale"] is True
    r = ok(c, {"type": "stream", "channel": "a", "turn": "t2", "delta": "Fresh.", "final": True})
    assert r["stale"] is False
    assert heard(c, 1) == [("Fresh.", "a")]


def test_the_pause_stays_on_when_another_channel_gets_a_new_turn(rt):
    # Both channels are read, so the case runs with policy "all" (#195:
    # under earcon_only a focused session's background peer is not read).
    c = agent_client(rt, policy="all")
    ok(c, {"type": "channel_open", "channel": "a"})
    ok(c, {"type": "channel_open", "channel": "b"})
    ok(c, {"type": "focus", "channel": "b"})
    stream(c, "a", LONG)
    assert starts_long(heard(c, 1)[0], "a")
    ok(c, {"type": "control", "action": "pause"})
    c.state(lambda s: s["paused"] is True)
    ok(c, {"type": "turn_start", "channel": "b", "t": 2.0})
    stream(c, "b", "Beta answer.", t=2.0)
    ok(c, {"type": "turn_end", "channel": "b", "t": 2.0})
    time.sleep(0.3)
    now = snapshot(rt)
    assert now["paused"] is True, now
    assert now["now_playing"]["channel"] == "a"
    # The engaged channel's own new turn resumes.
    ok(c, {"type": "turn_start", "channel": "a", "t": 3.0})
    c.state(lambda s: s["paused"] is False)
    stream(c, "a", "Alpha again.", t=3.0)
    got = heard(c, 2)
    assert ("Beta answer.", "b") in got and ("Alpha again.", "a") in got


def test_a_decision_is_read_before_the_rest_of_another_channels_turn(rt):
    c = agent_client(rt)
    stream(c, "a", TWO_LONG)
    assert starts_long(heard(c, 1)[0], "a", 0)
    ok(c, {"type": "ask", "channel": "b", "kind": "question", "text": "Deploy now?",
           "options": [{"label": "Yes", "description": "ship it"}, "No"]})
    assert earcon(c) == "choice"
    ok(c, {"type": "control", "action": "skip"})
    got = heard(c, 2)
    assert got[0] == ("Deploy now?", "b")
    assert starts_long(got[1], "a", 1)


def test_the_questions_own_permission_prompt_is_suppressed(rt):
    # #11: AskUserQuestion also fires a permission prompt seconds later.
    c = agent_client(rt)
    ok(c, {"type": "ask", "channel": "a", "kind": "question", "text": "Pick one?", "options": ["A"]})
    assert earcon(c) == "choice"
    assert heard(c, 1) == [("Pick one?", "a")]
    ok(c, {"type": "ask", "channel": "a", "kind": "permission", "text": "Claude needs your permission"})
    no_earcon(c)
    ok(c, {"type": "ask", "channel": "a", "kind": "permission", "text": "Run git status."})
    assert earcon(c) == "permission"
    assert heard(c, 1) == [("Run git status.", "a")]


def test_a_plan_has_no_earcon(rt):
    c = agent_client(rt)
    ok(c, {"type": "ask", "channel": "a", "kind": "plan", "text": "Add the parser."})
    assert heard(c, 1) == [("Plan ready.", "a")]
    no_earcon(c)


def test_turn_end_plays_turn_done_without_cutting_speech(rt):
    c = agent_client(rt)
    stream(c, "a", LONG)
    assert starts_long(heard(c, 1)[0], "a")
    ok(c, {"type": "turn_end", "channel": "a"})
    assert earcon(c) == "turn_done"
    ok(c, {"type": "earcon", "kind": "nav_edge"})
    assert earcon(c) == "nav_edge"
    assert snapshot(rt)["now_playing"]["channel"] == "a", "not cut"


def test_mute_levels(rt):
    c = agent_client(rt)
    stream(c, "a", LONG)
    (first,) = heard(c, 1)
    r = ok(c, {"type": "set", "key": "mute_level", "value": 1})
    assert r["value"] == 1
    c.state(lambda s: s["now_playing"] is None)
    stream(c, "a", "Muted words.", 1)
    ok(c, {"type": "turn_end", "channel": "a"})
    assert earcon(c) == "turn_done", "level 1 keeps the earcons"
    quiet(c)
    ok(c, {"type": "set", "key": "mute_level", "value": 2})
    ok(c, {"type": "turn_end", "channel": "a"})
    ok(c, {"type": "earcon", "kind": "nav"})
    no_earcon(c)
    assert c.request({"type": "set", "key": "mute_level", "value": 3})["error"]["code"] == "E_BAD_REQUEST"
    ok(c, {"type": "set", "key": "mute_level", "value": 0})
    assert ok(c, {"type": "get", "key": "mute_level"})["value"] == 0
    ok(c, {"type": "turn_start", "channel": "a"})
    stream(c, "a", "Heard again.")
    assert heard(c, 1) == [("Heard again.", "a")]


def test_tools_are_announced_and_an_answer_catches_up(rt):
    c = agent_client(rt)
    ok(c, {"type": "tool", "channel": "a", "name": "Bash", "summary": "git status"})
    assert heard(c, 1) == [("git status", "a")]
    ok(c, {"type": "set", "key": "verbosity", "value": "medium"})
    ok(c, {"type": "tool", "channel": "a", "name": "Bash", "summary": "ls"})
    quiet(c, 0.4)
    stream(c, "a", TWO_LONG)
    assert starts_long(heard(c, 1)[0], "a", 0)
    ok(c, {"type": "answered", "channel": "a"})
    c.state(lambda s: s["now_playing"] is None)
    quiet(c)
    stream(c, "a", "After the answer.", 1)
    assert heard(c, 1) == [("After the answer.", "a")]


def test_closing_a_channel_forgets_its_turn(rt):
    c = agent_client(rt)
    ok(c, {"type": "turn_start", "channel": "a", "t": 50.0})
    stream(c, "a", LONG, t=50.0)
    heard(c, 1)
    ok(c, {"type": "channel_close", "channel": "a"})
    c.state(lambda s: s["now_playing"] is None)
    # Reopened, the old turn's start time no longer applies.
    assert stream(c, "a", "Fresh start.", t=1.0)["stale"] is False
    assert heard(c, 1) == [("Fresh start.", "a")]


def test_agent_over_http(rt):
    status, r = rt.post("hello", {"extensions": ["agent"]})
    assert status == 200 and r["extensions"] == ["channels", "agent"]
    status, r = rt.post("stream", {"channel": "h", "delta": "Over HTTP.", "final": True})
    assert status == 200 and r["stale"] is False
    status, r = rt.post("ask", {"channel": "h", "kind": "riddle"})
    assert status == 400 and r["error"]["code"] == "E_BAD_REQUEST"
    status, r = rt.post("get", {"key": "summaries"})
    assert status == 200 and r["value"]["enabled"] is False


# -- background speech policy (#195) and per-channel mute (#196) ----------


def test_the_agent_defaults_are_the_product_defaults(rt):
    # #202: the maintainer's settings of the Python plugin, unmuted.
    c = rt.tcp(extensions=["agent"])
    for key, want in (("background_policy", "all"), ("verbosity", "medium"),
                      ("minqueue", 5), ("mute_level", 0)):
        assert ok(c, {"type": "get", "key": key})["value"] == want, key
    assert ok(c, {"type": "get", "key": "summaries"})["value"]["enabled"] is False
    r = c.request({"type": "set", "key": "background_policy", "value": "silent"})
    assert r["error"]["code"] == "E_BAD_REQUEST"


def test_earcon_only_reads_the_focused_session_and_chimes_for_the_others(rt):
    # sessions.py earcon_only: only the foreground session (the last one
    # prompted) is read; a background session plays its earcons and its
    # text waits until the user prompts it or switches to it.
    c = agent_client(rt)
    for ch in ("fg", "bg"):
        ok(c, {"type": "channel_open", "channel": ch})
    ok(c, {"type": "focus", "channel": "fg"})
    stream(c, "bg", "Background prose.")
    ok(c, {"type": "ask", "channel": "bg", "kind": "question", "text": "Background question?"})
    assert earcon(c) == "choice"
    ok(c, {"type": "turn_end", "channel": "bg"})
    assert earcon(c) == "turn_done"
    quiet(c)
    stream(c, "fg", "Foreground prose.")
    assert heard(c, 1) == [("Foreground prose.", "fg")]
    quiet(c)
    ok(c, {"type": "control", "action": "next_channel"})
    assert heard(c, 2) == [("Background prose.", "bg"), ("Background question?", "bg")]


def test_policy_all_reads_every_session(rt):
    c = agent_client(rt, policy="all")
    for ch in ("fg", "bg"):
        ok(c, {"type": "channel_open", "channel": ch})
    ok(c, {"type": "focus", "channel": "fg"})
    stream(c, "bg", "Background prose.")
    assert heard(c, 1) == [("Background prose.", "bg")]


def test_the_previous_focus_finishes_its_message_when_another_session_is_prompted(rt):
    # ingest.py cooperative hand-off.
    c = agent_client(rt)
    ok(c, {"type": "channel_open", "channel": "a"})
    ok(c, {"type": "channel_open", "channel": "b"})
    ok(c, {"type": "focus", "channel": "a"})
    stream(c, "a", "First sentence here. Second sentence here.")
    assert heard(c, 1) == [("First sentence here.", "a")]
    ok(c, {"type": "focus", "channel": "b"})
    stream(c, "b", "Beta answer.")
    assert heard(c, 2) == [("Second sentence here.", "a"), ("Beta answer.", "b")]


def test_a_muted_session_is_held_and_never_switched_to(rt):
    # #196 (router.py, session_prefs muted): its text waits unread, its
    # earcons play, a channel switch skips it; unmuted, it is read.
    c = agent_client(rt, policy="all")
    for ch in ("a", "m"):
        ok(c, {"type": "channel_open", "channel": ch})
    ok(c, {"type": "set", "key": "channel_prefs", "value": {"channel": "m", "muted": True}})
    stream(c, "m", "Muted words.")
    ok(c, {"type": "turn_end", "channel": "m"})
    assert earcon(c) == "turn_done"
    quiet(c)
    stream(c, "a", "Alpha words.")
    assert heard(c, 1) == [("Alpha words.", "a")]
    c.state(lambda s: s["now_playing"] is None)
    r = ok(c, {"type": "control", "action": "next_channel"})
    assert r["channel"] == "a", "the muted session never takes the floor"
    c.state(lambda s: s["now_playing"] is None)
    c.events.clear()
    ok(c, {"type": "set", "key": "channel_prefs", "value": {"channel": "m", "muted": False}})
    assert heard(c, 1) == [("Muted words.", "m")]


def test_muting_the_session_being_read_cuts_it(rt):
    c = agent_client(rt)
    stream(c, "a", LONG)
    assert starts_long(heard(c, 1)[0], "a")
    ok(c, {"type": "set", "key": "channel_prefs", "value": {"channel": "a", "muted": True}})
    c.state(lambda s: s["now_playing"] is None)
    quiet(c)


def test_forgetting_a_dead_session(rt):
    # #197 (Python forget_session): a session that died without SessionEnd.
    c = agent_client(rt)
    ok(c, {"type": "channel_open", "channel": "dead", "label": "old"})
    ok(c, {"type": "channel_open", "channel": "live"})
    ok(c, {"type": "focus", "channel": "live"})
    ok(c, {"type": "turn_start", "channel": "dead", "t": 50.0})
    r = c.request({"type": "set", "key": "channel_prefs", "value": {"channel": "live", "forget": True}})
    assert r["error"]["code"] == "E_BAD_REQUEST"
    r = ok(c, {"type": "set", "key": "channel_prefs", "value": {"channel": "dead", "forget": True}})
    assert [row["channel"] for row in r["value"]] == ["live"]
    assert c.request({"type": "focus", "channel": "dead"})["error"]["code"] == "E_NOT_FOUND"
    # Reopened, the old turn's start time no longer applies.
    assert stream(c, "dead", "Fresh start.", t=1.0)["stale"] is False
