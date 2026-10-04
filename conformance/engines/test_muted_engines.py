"""Muted means nothing leaves the PC (#227, ``docs/protocol-v1.md``
"External engines", Muted): while the reader is muted (``control mute``) or
the agent's ``mute_level`` is 1 or 2, no request of any kind reaches an
external engine. Text is read with the built-in voice (the fake engine
here), the mute cues are spoken locally, voice lists are not fetched, and
everything resumes once unmuted. The user's own Test button still reaches
the provider. A fake OpenAI-compatible server counts what it receives."""
from __future__ import annotations

import time

SECRET = "sk-conformance-secret-0123456789"
TEXT = "First sentence here. Second sentence here. Third sentence here."


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def use_profile(c, profile):
    ok(c, {"type": "engine_add", "engine": profile, "secret": SECRET})
    ok(c, {"type": "set", "key": "engine", "value": "local"})


def read(c, text=TEXT):
    item = ok(c, {"type": "speak", "text": text})["item_id"]
    c.item(item, "finished")


def cue(c, timeout=5.0):
    return c.next_event(lambda e: e.get("event") == "cue", timeout)["text"]


def test_core_mute_sends_nothing_and_unmute_resumes(client, profile, provider):
    use_profile(client, profile)
    ok(client, {"type": "subscribe", "events": ["items"]})
    ok(client, {"type": "control", "action": "mute"})
    read(client)
    r = ok(client, {"type": "voices", "engine": "local", "refresh": True})
    assert r["error"]["reason"] == "muted"
    assert provider.requests == [], "nothing reached the provider while muted"
    ok(client, {"type": "control", "action": "unmute"})
    read(client, "Back again.")
    assert len(provider.speech()) == 1, "requests resume after unmute"


def test_mute_levels_and_their_cues_send_nothing(rt, profile, provider):
    c = rt.tcp(extensions=["system", "agent"])
    use_profile(c, profile)
    ok(c, {"type": "subscribe", "events": ["items", "cues"]})
    for level, said in [(1, "Muted."), (2, "Super muted.")]:
        ok(c, {"type": "set", "key": "mute_level", "value": level})
        assert cue(c) == said
        read(c)
    ok(c, {"type": "set", "key": "mute_level", "value": 0})
    assert cue(c) == "Unmuted."
    assert provider.requests == [], "the cues and the text were read locally"
    read(c, "Back again.")
    assert len(provider.speech()) == 1


def test_muting_mid_message_stops_the_requests(rt, profile, provider):
    c = rt.tcp(extensions=["agent"])
    use_profile(c, profile)
    ok(c, {"type": "speak", "text": TEXT})
    deadline = time.time() + 5
    while not provider.speech():
        assert time.time() < deadline, "the first chunk never reached the provider"
        time.sleep(0.01)
    ok(c, {"type": "set", "key": "mute_level", "value": 1})
    before = len(provider.requests)
    time.sleep(0.5)
    assert len(provider.requests) == before, "nothing more after the mute"


def test_the_test_button_still_reaches_the_provider(client, profile, provider):
    use_profile(client, profile)
    ok(client, {"type": "control", "action": "mute"})
    ok(client, {"type": "engine_test", "engine": "local", "play": False})
    assert len(provider.speech()) == 1
