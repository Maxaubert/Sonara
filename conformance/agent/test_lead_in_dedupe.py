"""Extension ``agent``: the text before a question is heard once, whatever
order the hook's messages arrive in (audit 2026-11-23, #380). The hook's
``MessageDisplay`` may bring the text as a ``stream`` before ``PreToolUse``
sends it again as the question's ``lead_in``; and an ``ask`` delivered twice
(one ``tool_use_id``) is one question."""
from __future__ import annotations

import time

TEXT = "Two ways to go here, pick the one you like."


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def client(rt):
    """Agent on, subscribed to state and earcons, prose read at once."""
    c = rt.tcp(extensions=["agent"])
    assert c.request({"type": "subscribe", "events": ["state", "earcons"]})["ok"]
    c.state()
    ok(c, {"type": "set", "key": "channel_announce", "value": "off"})
    ok(c, {"type": "set", "key": "read_mode", "value": "immediate"})
    ok(c, {"type": "channel_open", "channel": "q", "label": "quiz"})
    ok(c, {"type": "focus", "channel": "q"})
    return c


def heard(c, count):
    """The next ``count`` items to start, as their first chunk's text."""
    out, last = [], None
    while len(out) < count:
        np = c.next_event(lambda e: e.get("event") == "state")["now_playing"]
        if np is not None and np["item_id"] != last:
            last = np["item_id"]
            out.append(np["text"])
    c.last_item = last
    return out


def nothing_new(c, last, seconds=1.0):
    """No item other than ``last`` starts for ``seconds``."""
    end = time.monotonic() + seconds
    while (left := end - time.monotonic()) > 0:
        try:
            e = c.next_event(lambda e: e.get("event") == "state", timeout=left)
        except AssertionError:
            return
        np = e["now_playing"]
        assert np is None or np["item_id"] == last, np


def earcons(c, seconds=1.0):
    """The earcons played in the next ``seconds``."""
    out, end = [], time.monotonic() + seconds
    while (left := end - time.monotonic()) > 0:
        try:
            out.append(c.next_event(lambda e: e.get("event") == "earcon", timeout=left)["kind"])
        except AssertionError:
            break
    return out


def ask(c, **extra):
    ok(c, {"type": "ask", "channel": "q", "kind": "question", "text": "Which one?",
           "options": ["A", "B"], **extra})


def test_text_streamed_then_sent_as_the_lead_in_is_heard_once(rt):
    c = client(rt)
    ok(c, {"type": "stream", "channel": "q", "delta": TEXT, "index": 0, "final": True})
    ok(c, {"type": "stream", "channel": "q", "delta": TEXT, "lead_in": True})
    ask(c)
    said = heard(c, 2)
    assert said[0] == TEXT
    assert said[1].startswith("Which one?"), said
    nothing_new(c, c.last_item)


def test_a_question_sent_twice_with_one_tool_use_id_is_heard_once(rt):
    c = client(rt)
    ask(c, tool_use_id="toolu_1")
    ask(c, tool_use_id="toolu_1")
    assert earcons(c) == ["choice"]
    assert heard(c, 1)[0].startswith("Which one?")
    nothing_new(c, c.last_item)
