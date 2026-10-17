"""Extension ``agent`` (spec 4.3): streamed text, turns, decisions, earcons
and mute levels, observed through state, item and earcon events. Parity
cases ported from the Python daemon tests that encode product rules: one
message always the last, late text after a new turn dropped (#174), the
pause stays on when another channel gets a new turn (upstream #69),
decisions spoken with priority, the question's permission prompt
suppressed (#11), mute levels (earcons stay at level 1)."""
from __future__ import annotations

import time

import pytest

from harness import long_text

LONG = long_text(1)
TWO_LONG = long_text(2)


def agent_client(rt, announce=False, policy="earcon_only"):
    """A TCP client with the agent extension on, subscribed to state, items
    and earcons, past the first (idle) state. The parity cases run on the
    Python plugin's settings (prose read at once, every kind of text,
    ``policy`` as the background speech policy, default ``earcon_only``),
    not on the product defaults of #202 and #222 (the turn held until it is
    done, ``skip_code``, ``all``)."""
    c = rt.tcp(extensions=["agent"])
    assert c.request({"type": "subscribe", "events": ["state", "items", "earcons"]})["ok"]
    c.state()
    ok(c, {"type": "set", "key": "channel_announce", "value": "on" if announce else "off"})
    ok(c, {"type": "set", "key": "read_mode", "value": "immediate"})
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


def chunks_heard(c, count):
    """The next ``count`` chunks to play, as their text (#295: each code
    line is a chunk of the block's item)."""
    out, last = [], None
    while len(out) < count:
        e = c.next_event(lambda e: e.get("event") == "state")
        np = e["now_playing"]
        if np is None:
            continue
        at = (np["item_id"], np["chunk"])
        if at != last:
            last = at
            out.append(np["text"])
    c.last_item = last[0]
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
        {"type": "get", "key": "code_speed"},
        {"type": "set", "key": "code_step", "value": "on"},
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


def test_deltas_that_arrive_out_of_order_are_read_in_index_order(rt):
    # #286: each delta is its own hook process, so a block's deltas race
    # (seen: 0, 2 final, 1). They are read in index order.
    c = agent_client(rt)
    ok(c, {"type": "turn_start", "channel": "a", "t": 1.0})
    stream(c, "a", "Both work. ", 0, final=False)
    stream(c, "a", "Public part.", 2)
    stream(c, "a", "Local only. ", 1, final=False)
    assert heard(c, 3) == [("Both work.", "a"), ("Local only.", "a"), ("Public part.", "a")]
    quiet(c)


def test_deltas_after_the_turn_end_are_read_in_index_order(rt):
    # #286: the turn_end races the last deltas too (#14); they are ordered
    # the same way.
    c = agent_client(rt)
    ok(c, {"type": "turn_start", "channel": "a", "t": 1.0})
    ok(c, {"type": "turn_end", "channel": "a", "t": 1.0})
    stream(c, "a", "Second part.", 1)
    stream(c, "a", "First part. ", 0, final=False)
    assert heard(c, 2) == [("First part.", "a"), ("Second part.", "a")]
    quiet(c)


def test_a_missing_delta_never_holds_back_the_rest(rt):
    # #286: a delta ahead of a missing one waits briefly for it, then is read.
    c = agent_client(rt)
    ok(c, {"type": "turn_start", "channel": "a", "t": 1.0})
    stream(c, "a", "First part. ", 0, final=False)
    assert heard(c, 1) == [("First part.", "a")]
    stream(c, "a", "Third part.", 2)
    assert heard(c, 1) == [("Third part.", "a")]
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


def test_a_decision_waits_for_the_message_being_read(rt):
    # #305: the other session's message is read to its end first.
    c = agent_client(rt)
    stream(c, "a", TWO_LONG)
    assert starts_long(heard(c, 1)[0], "a", 0)
    ok(c, {"type": "ask", "channel": "b", "kind": "question", "text": "Deploy now?",
           "options": [{"label": "Yes", "description": "ship it"}, "No"]})
    assert earcon(c) == "choice"
    ok(c, {"type": "control", "action": "skip"})
    got = heard(c, 2)
    assert starts_long(got[0], "a", 1)
    assert got[1] == ("Deploy now?", "b")


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


def test_the_log_of_2026_10_05_muted_turns_are_stored_and_read_on_a_switch(rt):
    # #243: a turn ended while super muted was never stored, and after the
    # unmute next_channel replayed an older batch with a question that had
    # already been answered. Now: what comes while muted is stored as each
    # session's latest message, the unmute reads nothing, a switch or Up
    # reads the stored message, an answered question is never replayed.
    c = agent_client(rt)
    ok(c, {"type": "set", "key": "read_mode", "value": "done"})
    for ch, label in (("filesmith", "Filesmith"), ("web", "web-application-project"),
                      ("app", "app")):
        ok(c, {"type": "channel_open", "channel": ch, "label": label})
    ok(c, {"type": "turn_start", "channel": "web"})
    ok(c, {"type": "ask", "channel": "web", "kind": "question", "text": "Question 9b?"})
    assert heard(c, 1) == [("Question 9b?", "web")]
    c.state(lambda s: s["now_playing"] is None)
    ok(c, {"type": "turn_start", "channel": "filesmith"})
    stream(c, "filesmith", "Filesmith old.")
    ok(c, {"type": "turn_end", "channel": "filesmith"})
    assert heard(c, 1) == [("Filesmith old.", "filesmith")]
    c.state(lambda s: s["now_playing"] is None)
    ok(c, {"type": "answered", "channel": "web"})
    ok(c, {"type": "set", "key": "mute_level", "value": 2})
    stream(c, "web", "Done with 9b.")
    ok(c, {"type": "turn_end", "channel": "web"})
    ok(c, {"type": "turn_start", "channel": "app"})
    stream(c, "app", "App reply.")
    ok(c, {"type": "turn_end", "channel": "app"})
    quiet(c)
    ok(c, {"type": "set", "key": "mute_level", "value": 0})
    quiet(c, 1.0)
    assert ok(c, {"type": "control", "action": "next_channel"})["channel"] == "web"
    assert heard(c, 1) == [("Done with 9b.", "web")], "the latest message, not the question"
    c.state(lambda s: s["now_playing"] is None)
    quiet(c)
    assert ok(c, {"type": "control", "action": "next_channel"})["channel"] == "app"
    assert heard(c, 1) == [("App reply.", "app")]
    c.state(lambda s: s["now_playing"] is None)
    ok(c, {"type": "control", "action": "restart"})
    assert heard(c, 1) == [("App reply.", "app")]


def test_tools_are_announced_and_an_answer_catches_up(rt):
    c = agent_client(rt)
    ok(c, {"type": "tool", "channel": "a", "name": "Bash", "summary": "git status"})
    assert heard(c, 1) == [("git status", "a")]
    ok(c, {"type": "set", "key": "verbosity", "value": "skip_code"})
    ok(c, {"type": "tool", "channel": "a", "name": "Bash", "summary": "ls"})
    quiet(c, 0.4)
    stream(c, "a", TWO_LONG)
    assert starts_long(heard(c, 1)[0], "a", 0)
    ok(c, {"type": "answered", "channel": "a"})
    c.state(lambda s: s["now_playing"] is None)
    quiet(c)
    stream(c, "a", "After the answer.", 1)
    assert heard(c, 1) == [("After the answer.", "a")]


CODE_TURN = "Look.\n```python\nx = 1\ny = 2\n```\nDone.\n"


CODE_CHUNKS = [
    "Look.",
    "Python code block, 2 lines.",
    "Line one: x, equals, 1.",
    "Line two: y, equals, 2.",
    "End of Python code.",
    "Done.",
]


def test_everything_reads_code_blocks_and_skip_code_drops_them(rt):
    # #214: two levels. A code block is announced at everything and
    # skipped without a word at skip_code. #290, #295: at everything the
    # block is then read line by line, labelled, symbols in words, and
    # closed by name; the block is one item, each line a chunk.
    c = agent_client(rt)
    stream(c, "a", CODE_TURN)
    assert chunks_heard(c, 6) == CODE_CHUNKS
    c.state(lambda s: s["now_playing"] is None)
    ok(c, {"type": "set", "key": "verbosity", "value": "skip_code"})
    ok(c, {"type": "turn_start", "channel": "a"})
    stream(c, "a", CODE_TURN)
    assert heard(c, 2) == [("Look.", "a"), ("Done.", "a")]
    c.state(lambda s: s["now_playing"] is None)
    quiet(c)


def test_read_code_reads_code_blocks(rt):
    # #292: the step between skip_code and everything reads code.
    c = agent_client(rt)
    assert ok(c, {"type": "set", "key": "verbosity", "value": "read_code"})["value"] == "read_code"
    stream(c, "a", CODE_TURN)
    assert chunks_heard(c, 6) == CODE_CHUNKS
    c.state(lambda s: s["now_playing"] is None)
    quiet(c)


def test_step_through_code_waits_at_each_line(rt):
    # #295: with code_step on, the reader waits after the announcement and
    # after each line; next_line reads one line, previous_line the one
    # before, toggle the rest straight through.
    c = agent_client(rt)
    assert ok(c, {"type": "set", "key": "code_step", "value": "on"})["value"] == "on"
    stream(c, "a", CODE_TURN)
    s = c.state(lambda s: s["paused"] and s["now_playing"] is not None)
    assert s["now_playing"]["text"] == "Line one: x, equals, 1."
    ok(c, {"type": "control", "action": "next_line"})
    s = c.state(lambda s: s["paused"] and (s["now_playing"] or {}).get("chunk") == 2)
    assert s["now_playing"]["text"] == "Line two: y, equals, 2."
    ok(c, {"type": "control", "action": "previous_line"})
    c.state(lambda s: s["paused"] and (s["now_playing"] or {}).get("chunk") == 2)
    ok(c, {"type": "control", "action": "toggle"})
    c.state(lambda s: (s["now_playing"] or {}).get("text") == "Done.")
    c.state(lambda s: s["now_playing"] is None)
    c.events.clear()  # the states skipped while waiting above
    quiet(c)


def test_line_controls_outside_code_change_nothing(rt):
    # #295: next_line and previous_line act only in a code block.
    c = agent_client(rt)
    for action in ("next_line", "previous_line"):
        ok(c, {"type": "control", "action": action})
    assert c.request({"type": "get", "key": "code_speed"})["value"] == 80
    assert c.request({"type": "set", "key": "code_speed", "value": 30})["error"]["code"] == "E_BAD_REQUEST"


def test_old_verbosity_names_are_accepted_as_aliases(rt):
    # #214: older clients send the three-level names.
    c = rt.tcp(extensions=["agent"])
    for old, now in (("medium", "skip_code"), ("quiet", "skip_code"), ("all", "everything")):
        assert ok(c, {"type": "set", "key": "verbosity", "value": old})["value"] == now, old
    assert c.request({"type": "set", "key": "verbosity", "value": "loud"})["error"]["code"] == "E_BAD_REQUEST"


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


# -- reading mode (#222) ----------------------------------------------------


def test_read_mode_is_set_and_got_and_refuses_other_values(rt):
    c = rt.tcp(extensions=["agent"])
    assert ok(c, {"type": "get", "key": "read_mode"})["value"] == "done"
    for mode in ("immediate", "queue", "done"):
        assert ok(c, {"type": "set", "key": "read_mode", "value": mode})["value"] == mode
        assert ok(c, {"type": "get", "key": "read_mode"})["value"] == mode
    for bad in ("later", 1, None):
        r = c.request({"type": "set", "key": "read_mode", "value": bad})
        assert r["error"]["code"] == "E_BAD_REQUEST", bad
    assert ok(c, {"type": "get", "key": "read_mode"})["value"] == "done"


def test_read_mode_done_holds_the_turn_through_a_tool_until_it_ends(rt):
    c = agent_client(rt)
    ok(c, {"type": "set", "key": "read_mode", "value": "done"})
    ok(c, {"type": "turn_start", "channel": "a"})
    stream(c, "a", "First part.", 0)
    ok(c, {"type": "tool", "channel": "a", "name": "Bash", "summary": "ls"})
    assert heard(c, 1) == [("ls", "a")], "the tool is announced, the prose held"
    c.state(lambda s: s["now_playing"] is None)
    stream(c, "a", "Second part.", 1)
    quiet(c)
    ok(c, {"type": "turn_end", "channel": "a"})
    assert heard(c, 2) == [("First part.", "a"), ("Second part.", "a")]


def test_read_mode_done_reads_the_held_prose_before_a_question(rt):
    c = agent_client(rt)
    ok(c, {"type": "set", "key": "read_mode", "value": "done"})
    ok(c, {"type": "turn_start", "channel": "a"})
    stream(c, "a", "Two ways to go.")
    quiet(c, 0.4)
    ok(c, {"type": "ask", "channel": "a", "kind": "question", "text": "Which one?",
           "options": ["A", "B"]})
    assert earcon(c) == "choice"
    first, second = heard(c, 2)
    assert first == ("Two ways to go.", "a")
    assert second[0].startswith("Which one?") and second[1] == "a"


def test_read_mode_queue_holds_until_the_queue_size(rt):
    c = agent_client(rt)
    ok(c, {"type": "set", "key": "read_mode", "value": "queue"})
    ok(c, {"type": "set", "key": "minqueue", "value": 3})
    ok(c, {"type": "turn_start", "channel": "a"})
    stream(c, "a", "One. Two. ", 0, final=False)
    quiet(c, 0.4)
    stream(c, "a", "Three.", 1)
    assert heard(c, 1) == [("One.", "a")]


# -- background speech policy (#195) and per-channel mute (#196) ----------


def test_the_agent_defaults_are_the_product_defaults(rt):
    # #202: the maintainer's settings of the Python plugin, unmuted.
    c = rt.tcp(extensions=["agent"])
    for key, want in (("background_policy", "all"), ("verbosity", "skip_code"),
                      ("read_mode", "done"), ("minqueue", 5), ("mute_level", 0)):
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


def test_flush_stops_only_the_session_being_read(rt):
    # #228: flushing the session being read keeps the turn another session
    # is still streaming (held by read_mode done here) and its summary work.
    c = agent_client(rt, policy="all")
    for ch in ("a", "b"):
        ok(c, {"type": "channel_open", "channel": ch})
    ok(c, {"type": "set", "key": "read_mode", "value": "done"})
    stream(c, "b", "Beta still arriving.", final=False)
    stream(c, "a", LONG)
    ok(c, {"type": "turn_end", "channel": "a"})
    assert starts_long(heard(c, 1)[0], "a")
    r = ok(c, {"type": "control", "action": "flush"})
    assert (r["flushed"], r["channel"]) == ("channel", "a")
    quiet(c)
    stream(c, "b", " Done now.", final=True)
    ok(c, {"type": "turn_end", "channel": "b"})
    assert heard(c, 2) == [("Beta still arriving.", "b"), ("Done now.", "b")]


# -- flush scope (#228, 2026-10-04) ------------------------------------------


def test_flush_scope_is_set_and_got_and_refuses_other_values(rt):
    c = rt.tcp(extensions=["agent"])
    assert ok(c, {"type": "get", "key": "flush_scope"})["value"] == "session"
    for scope in ("all", "session"):
        assert ok(c, {"type": "set", "key": "flush_scope", "value": scope})["value"] == scope
        assert ok(c, {"type": "get", "key": "flush_scope"})["value"] == scope
    for bad in ("everything", 1, None):
        r = c.request({"type": "set", "key": "flush_scope", "value": bad})
        assert r["error"]["code"] == "E_BAD_REQUEST", bad


def test_question_step_is_off_by_default_and_every_question_is_read(rt):
    # #320 (protocol 1.11): off reads a set's questions in order.
    c = agent_client(rt)
    assert ok(c, {"type": "get", "key": "question_step"})["value"] == "off"
    for i in range(2):
        ok(c, {"type": "ask", "channel": "a", "kind": "question", "text": f"Pick {i + 1}?",
               "options": ["Yes"], "set_index": i, "set_size": 2})
    assert [t for t, _ in heard(c, 2)] == ["Question 1 of 2.", "Question 2 of 2."]
    for v in ("on", "off"):
        assert ok(c, {"type": "set", "key": "question_step", "value": v})["value"] == v
    for bad in ("maybe", True, None):
        r = c.request({"type": "set", "key": "question_step", "value": bad})
        assert r["error"]["code"] == "E_BAD_REQUEST", bad


def test_flush_skips_the_rest_of_the_flushed_reply_but_reads_its_question(rt):
    c = agent_client(rt, policy="all")
    ok(c, {"type": "channel_open", "channel": "a"})
    ok(c, {"type": "turn_start", "channel": "a"})
    stream(c, "a", LONG)
    assert starts_long(heard(c, 1)[0], "a")
    r = ok(c, {"type": "control", "action": "flush"})
    assert (r["flushed"], r["scope"], r["others"]) == ("channel", "session", [])
    quiet(c)
    stream(c, "a", "Skipped too.", index=1)
    quiet(c)
    ok(c, {"type": "ask", "channel": "a", "kind": "question", "text": "Deploy now?"})
    assert heard(c, 1)[0][1] == "a"
    ok(c, {"type": "turn_start", "channel": "a"})
    stream(c, "a", "Next reply.")
    assert heard(c, 1) == [("Next reply.", "a")]


@pytest.mark.parametrize("scope", ["session", "all"])
def test_flush_keeps_a_reply_another_session_is_still_writing(rt, scope):
    # #228, the incident, in both scopes.
    c = agent_client(rt, policy="all")
    for ch in ("a", "b", "z"):
        ok(c, {"type": "channel_open", "channel": ch})
    ok(c, {"type": "set", "key": "read_mode", "value": "done"})
    ok(c, {"type": "set", "key": "flush_scope", "value": scope})
    ok(c, {"type": "turn_start", "channel": "b"})
    stream(c, "b", "Beta still arriving.", final=False)
    stream(c, "a", LONG)
    ok(c, {"type": "turn_end", "channel": "a"})
    stream(c, "z", "Zeta finished.")
    ok(c, {"type": "turn_end", "channel": "z"})
    assert starts_long(heard(c, 1)[0], "a")
    r = ok(c, {"type": "control", "action": "flush"})
    assert (r["flushed"], r["channel"], r["scope"]) == ("channel", "a", scope)
    if scope == "all":
        assert r["others"] == ["z"]
    else:
        assert r["others"] == []
        assert heard(c, 1) == [("Zeta finished.", "z")]
    quiet(c)
    stream(c, "b", " Done now.", final=True)
    ok(c, {"type": "turn_end", "channel": "b"})
    assert heard(c, 2) == [("Beta still arriving.", "b"), ("Done now.", "b")]


def replaying_a(rt):
    """Session a's two-sentence reply was heard, then ``restart`` while idle
    (the Up key) replays it: its first sentence is playing again."""
    c = agent_client(rt)
    stream(c, "a", TWO_LONG)
    assert starts_long(heard(c, 1)[0], "a", 0)
    ok(c, {"type": "control", "action": "skip"})
    assert starts_long(heard(c, 1)[0], "a", 1)
    ok(c, {"type": "control", "action": "skip"})
    c.state(lambda s: s["now_playing"] is None)
    ok(c, {"type": "control", "action": "restart"})
    assert starts_long(heard(c, 1)[0], "a", 0)
    return c


def ask_b(c):
    ok(c, {"type": "ask", "channel": "b", "kind": "question", "text": "Deploy now?",
           "options": ["Yes"]})
    assert earcon(c) == "choice"


def test_a_replay_is_not_taken_over_by_another_sessions_question(rt):
    # #271: the replay the user started holds the floor until its batch
    # ends; the question waits behind it.
    c = replaying_a(rt)
    ask_b(c)
    ok(c, {"type": "control", "action": "skip"})
    assert starts_long(heard(c, 1)[0], "a", 1)
    ok(c, {"type": "control", "action": "skip"})
    assert heard(c, 1) == [("Deploy now?", "b")]


def test_flush_during_a_replay_moves_on(rt):
    c = replaying_a(rt)
    ask_b(c)
    r = ok(c, {"type": "control", "action": "flush"})
    assert (r["flushed"], r["channel"]) == ("channel", "a")
    assert heard(c, 1) == [("Deploy now?", "b")]


# -- question sets (#283) -----------------------------------------------------

def ask_set(c, channel, n):
    """A set of ``n`` questions as the Claude hook sends one AskUserQuestion,
    with ``question_step`` on (#283; off by default since #320)."""
    ok(c, {"type": "set", "key": "question_step", "value": "on"})
    for i in range(n):
        ok(c, {"type": "ask", "channel": channel, "kind": "question", "text": f"Pick {i + 1}?",
               "options": ["Yes"], "set_index": i, "set_size": n})


def nav(c, action):
    return ok(c, {"type": "control", "action": action})


def until_idle(c):
    """Take the state events in order until nothing plays."""
    while c.state()["now_playing"] is not None:
        pass


def test_a_question_set_reads_only_its_first_question(rt):
    c = agent_client(rt)
    stream(c, "a", LONG)
    ask_set(c, "a", 4)
    assert earcon(c) == "choice"
    no_earcon(c)
    assert starts_long(heard(c, 1)[0], "a", 0)
    ok(c, {"type": "control", "action": "skip"})
    assert heard(c, 1) == [("Question 1 of 4.", "a")]
    until_idle(c)
    quiet(c)


def test_restart_with_a_question_set_reads_the_text_then_question_one(rt):
    # sonarad.log 2026-10-07 18:03:14: Up during question 2 restarted
    # question 2. With the set it reads the message again from the top.
    c = agent_client(rt)
    stream(c, "a", LONG)
    ask_set(c, "a", 4)
    assert starts_long(heard(c, 1)[0], "a", 0)
    ok(c, {"type": "control", "action": "skip"})
    assert heard(c, 1) == [("Question 1 of 4.", "a")]
    r = nav(c, "next_question")
    assert (r["channel"], r["question"], r["of"], r["edge"]) == ("a", 2, 4, False)
    assert heard(c, 1) == [("Question 2 of 4.", "a")]
    ok(c, {"type": "control", "action": "restart"})
    assert starts_long(heard(c, 1)[0], "a", 0)
    ok(c, {"type": "control", "action": "skip"})
    assert heard(c, 1) == [("Question 1 of 4.", "a")]


def test_next_and_previous_question_replies(rt):
    c = agent_client(rt)
    # No question set yet: nothing to navigate.
    assert nav(c, "next_question")["channel"] is None
    stream(c, "a", LONG)
    ask_set(c, "a", 2)
    assert starts_long(heard(c, 1)[0], "a", 0)
    # From the text, either key goes to question 1.
    r = nav(c, "previous_question")
    assert (r["channel"], r["question"], r["of"], r["edge"]) == ("a", 1, 2, False)
    assert heard(c, 1) == [("Question 1 of 2.", "a")]
    assert nav(c, "next_question")["question"] == 2
    r = nav(c, "next_question")
    assert (r["question"], r["edge"]) == (2, True)
    assert nav(c, "previous_question")["question"] == 1
    ok(c, {"type": "answered", "channel": "a"})
    assert nav(c, "next_question")["channel"] is None


def test_question_actions_need_the_agent_extension(rt):
    plain = rt.tcp()
    channels = rt.tcp(extensions=["channels"])
    for c in (plain, channels):
        for action in ("next_question", "previous_question"):
            r = c.request({"type": "control", "action": action})
            assert r["error"]["code"] == "E_UNSUPPORTED", (action, r)


def test_ask_set_fields_are_checked(rt):
    c = agent_client(rt)
    for bad in ({"set_index": 0}, {"set_index": 2, "set_size": 2}, {"set_index": -1, "set_size": 2}):
        r = c.request({"type": "ask", "channel": "a", "kind": "question", "text": "Q?", **bad})
        assert r["error"]["code"] == "E_BAD_REQUEST", (bad, r)


def test_question_shown_follows_the_question_on_screen(rt):
    # #323 (protocol 1.12): the terminal reports the question it shows.
    c = agent_client(rt)
    ask_set(c, "a", 3)
    assert heard(c, 1)[0][0] == "Question 1 of 3."
    r = ok(c, {"type": "question_shown", "channel": "a", "text": "  pick 3? "})
    assert (r["moved"], r["question"], r["of"]) == (True, 3, 3)
    assert heard(c, 1)[0][0] == "Question 3 of 3."
    assert ok(c, {"type": "question_shown", "channel": "a", "text": "Pick 3?"})["moved"] is False
    assert ok(c, {"type": "question_shown", "channel": "a", "text": "Not a question here"})["moved"] is False
    r = ok(c, {"type": "question_shown", "channel": "a", "review": True})
    assert r["moved"] is True and r["review"] is True
    assert ok(c, {"type": "question_shown", "channel": "a", "review": True})["moved"] is False
    for bad in ({"text": "Pick 1?", "review": True}, {}, {"review": "yes"}):
        r = c.request({"type": "question_shown", "channel": "a", **bad})
        assert r["error"]["code"] == "E_BAD_REQUEST", bad

def test_question_shown_reads_only_the_question_on_screen(rt):
    # #323, owner 2026-10-16: with question_step off, a set the screen is
    # reported for reads the question on screen alone, never the rest.
    c = agent_client(rt)
    for i in range(3):
        ok(c, {"type": "ask", "channel": "a", "kind": "question", "text": f"Pick {i + 1}?",
               "options": ["Yes"], "set_index": i, "set_size": 3})
    assert heard(c, 1)[0][0] == "Question 1 of 3."
    r = ok(c, {"type": "question_shown", "channel": "a", "text": "Pick 2?"})
    assert (r["moved"], r["question"], r["of"]) == (True, 2, 3)
    assert heard(c, 1)[0][0] == "Question 2 of 3."
    until_idle(c)
    quiet(c)
    assert ok(c, {"type": "question_shown", "channel": "a", "text": "Pick 2?"})["moved"] is False
    quiet(c)
    r = ok(c, {"type": "question_shown", "channel": "a", "text": "Pick 1?"})
    assert (r["moved"], r["question"]) == (True, 1)
    assert heard(c, 1)[0][0] == "Question 1 of 3."
    until_idle(c)
    quiet(c)


def test_question_shown_finds_the_session_by_its_terminal_tab(rt):
    # #323: PrismTerminal knows its tab, not the Claude session; the hook
    # reported the tab as the channel's host_tab.
    c = agent_client(rt)
    ok(c, {"type": "channel_open", "channel": "old", "host_tab": "tab-9"})
    ok(c, {"type": "channel_open", "channel": "t", "host_tab": "tab-9"})
    ask_set(c, "t", 2)
    assert heard(c, 1)[0][0] == "Question 1 of 2."
    r = ok(c, {"type": "question_shown", "host_tab": "tab-9", "text": "Pick 2?"})
    assert (r["moved"], r["channel"], r["question"]) == (True, "t", 2)
    assert ok(c, {"type": "question_shown", "host_tab": "tab-0", "text": "Pick 1?"})["moved"] is False
    r = c.request({"type": "question_shown", "text": "Pick 1?"})
    assert r["error"]["code"] == "E_BAD_REQUEST"