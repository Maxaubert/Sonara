"""Extension ``agent``: a question's lead-in that comes after it (protocol
1.16, #360). Claude Code writes a question to its transcript only after the
hook, so the hook marks the ``ask`` ``lead_in_pending`` and a follow-up sends
the ``stream`` ``lead_in`` later. The question waits for it (at most 2 s), is
read after it, and goes at once on an empty one."""
from __future__ import annotations

import time

from test_agent import agent_client, earcon, heard, ok, quiet, stream


def pending_ask(c, ch, text):
    ok(c, {"type": "ask", "channel": ch, "kind": "question", "text": text,
           "options": ["Yes", "No"], "lead_in_pending": True, "lead_in_id": "q1"})


def lead_in(c, ch, delta, lead_in_id="q1"):
    ok(c, {"type": "stream", "channel": ch, "delta": delta, "lead_in": True,
           "lead_in_id": lead_in_id})


def test_the_question_waits_for_its_late_lead_in_and_is_read_after_it(rt):
    c = agent_client(rt)
    ok(c, {"type": "channel_open", "channel": "w", "label": "wallet"})
    ok(c, {"type": "focus", "channel": "w"})
    pending_ask(c, "w", "Upload these 10 files?")
    assert earcon(c) == "choice", "the chime is instant"
    quiet(c, 0.5)
    lead_in(c, "w", "It's ten files, not eleven.")
    said = heard(c, 2)
    assert said[0] == ("It's ten files, not eleven.", "w")
    assert said[1][0].startswith("Upload these 10 files?")


def test_an_empty_late_lead_in_lets_the_question_go_at_once(rt):
    c = agent_client(rt)
    ok(c, {"type": "channel_open", "channel": "w", "label": "wallet"})
    ok(c, {"type": "focus", "channel": "w"})
    pending_ask(c, "w", "Go on?")
    quiet(c, 0.3)
    lead_in(c, "w", "")
    assert heard(c, 1)[0][0].startswith("Go on?")


def test_without_its_lead_in_the_question_is_read_after_two_seconds(rt):
    c = agent_client(rt)
    ok(c, {"type": "channel_open", "channel": "w", "label": "wallet"})
    ok(c, {"type": "focus", "channel": "w"})
    start = time.monotonic()
    pending_ask(c, "w", "Go on?")
    assert heard(c, 1)[0][0].startswith("Go on?")
    assert time.monotonic() - start >= 1.9
    # Too late now: a lead-in that still comes is not read after it.
    c.state(lambda s: s["now_playing"] is None)
    lead_in(c, "w", "Context.")
    stream(c, "w", "Later words.")
    assert heard(c, 1) == [("Later words.", "w")]


def test_a_late_lead_in_of_another_question_is_dropped(rt):
    c = agent_client(rt)
    ok(c, {"type": "channel_open", "channel": "w", "label": "wallet"})
    ok(c, {"type": "focus", "channel": "w"})
    pending_ask(c, "w", "Go on?")
    lead_in(c, "w", "Old context.", lead_in_id="q0")
    quiet(c, 0.4)
    lead_in(c, "w", "New context.")
    said = heard(c, 2)
    assert said[0] == ("New context.", "w")
    assert said[1][0].startswith("Go on?")
