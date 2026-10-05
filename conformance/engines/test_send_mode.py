"""Send to the engine (#235, ``docs/protocol-v1-engines.md`` "External engines",
``send_mode``): with a cloud profile in send mode ``message`` (the cloud
default) the agent's reply in read mode ``done`` is ONE request to the
provider; in ``sentence`` mode it is one request per sentence (the
evidence of 2026-10-04: 18 sentences, 18 Gemini requests). A flush, a
skip, a new turn or a mute while the message streams in ends the request:
the provider sees the connection closed and gets nothing more. The
profile view tells the send mode in force and whether the user chose it.
A fake ElevenLabs counts what it receives; no real provider is called."""
from __future__ import annotations

import time

import pytest
from fakes import FakeCloud
from harness import wait_until

SECRET = "cloud-conformance-secret-0123456789"
WORDS = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
         "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen"]
SENTENCES = [f"This is line {w}." for w in WORDS]
REPLY = " ".join(SENTENCES[:9]) + "\n\n" + " ".join(SENTENCES[9:])


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


@pytest.fixture
def cloud():
    fake = FakeCloud("elevenlabs", SECRET)
    yield fake
    fake.stop()


def agent(rt, cloud, send_mode=None):
    """An agent client reading with the fake ElevenLabs, read mode done."""
    c = rt.tcp(extensions=["agent"])
    profile = cloud.profile()
    if send_mode:
        profile["send_mode"] = send_mode
    view = ok(c, {"type": "engine_add", "engine": profile, "secret": SECRET, "replace": True})["engine"]
    ok(c, {"type": "set", "key": "engine", "value": view["id"]})
    ok(c, {"type": "set", "key": "read_mode", "value": "done"})
    ok(c, {"type": "set", "key": "channel_announce", "value": "off"})
    ok(c, {"type": "subscribe", "events": ["state", "items"]})
    return c, view


def reply(c, channel="a"):
    """A whole turn: 18 sentences in two paragraphs, a sentence per delta."""
    ok(c, {"type": "turn_start", "channel": channel})
    for s in REPLY.split(". "):
        delta = s if s.endswith(".") else s + ". "
        ok(c, {"type": "stream", "channel": channel, "delta": delta, "index": 0, "final": False})
    ok(c, {"type": "stream", "channel": channel, "delta": "", "index": 0, "final": True})
    ok(c, {"type": "turn_end", "channel": channel})


def settled(cloud, n, seconds=0.8):
    """Wait for ``n`` speech requests, then make sure no more come."""
    assert wait_until(lambda: len(cloud.speech()) >= n), len(cloud.speech())
    time.sleep(seconds)
    return len(cloud.speech())


def test_the_view_tells_the_send_mode_and_whether_it_was_chosen(rt, cloud):
    c = rt.tcp()
    view = ok(c, {"type": "engine_add", "engine": cloud.profile(), "secret": SECRET})["engine"]
    assert view["send_mode"] == "message"
    assert "send_mode" not in view["explicit"]
    profile = dict(cloud.profile(), send_mode="sentence")
    view = ok(c, {"type": "engine_add", "engine": profile, "replace": True})["engine"]
    assert view["send_mode"] == "sentence"
    assert view["explicit"]["send_mode"] == "sentence"
    listed = ok(c, {"type": "engine_list"})["engines"][0]
    assert listed["send_mode"] == "sentence"
    bad = c.request({"type": "engine_add", "engine": dict(profile, send_mode="whole"), "replace": True})
    assert bad["error"]["code"] == "E_BAD_REQUEST"


def test_a_done_reply_is_one_request_in_message_mode(rt, cloud):
    c, _ = agent(rt, cloud)
    reply(c)
    assert settled(cloud, 1) == 1
    body = cloud.speech()[0]["body"].decode()
    assert "This is line one." in body and "This is line eighteen." in body
    assert "\\n\\n" in body, "the paragraph break is kept"
    assert cloud.speech()[0]["path"].startswith("/v1/text-to-speech/voice-a/stream?")


def test_a_done_reply_is_one_request_per_sentence_in_sentence_mode(rt, cloud):
    c, _ = agent(rt, cloud, "sentence")
    reply(c)
    assert settled(cloud, 18) == 18
    assert all("/stream" not in r["path"] for r in cloud.speech())


@pytest.mark.parametrize("action", ["flush", "skip", "turn_start", "mute"])
def test_a_message_streaming_in_is_cut_and_nothing_more_is_sent(rt, cloud, action):
    # 200 pieces of 0.1 s, 50 ms apart: 10 s of speech made over 10 s.
    cloud.slowly(200, 0.05)
    c, _ = agent(rt, cloud)
    reply(c)
    assert wait_until(lambda: len(cloud.speech()) == 1)
    c.next_event(lambda e: e.get("event") == "state" and e.get("now_playing") is not None)
    time.sleep(0.3)
    if action == "flush":
        ok(c, {"type": "control", "action": "flush"})
    elif action == "skip":
        ok(c, {"type": "control", "action": "skip"})
    elif action == "turn_start":
        ok(c, {"type": "turn_start", "channel": "a"})
    else:
        ok(c, {"type": "set", "key": "mute_level", "value": 1})
    assert wait_until(lambda: cloud.streams, timeout=8.0), "the answer never ended"
    written, cut = cloud.streams[0]
    assert cut, "the provider saw the connection closed"
    assert written < 200, written
    time.sleep(0.5)
    assert len(cloud.speech()) == 1, "nothing more was asked for"
