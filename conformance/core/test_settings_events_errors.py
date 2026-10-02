"""Core: set/get, voices, event streams over TCP and SSE, error codes and
unknown fields (spec 4, 4.1)."""
from __future__ import annotations

import pytest
from harness import SHORT_SENTENCE, long_text


@pytest.mark.parametrize(
    "key,value",
    [("volume", 40), ("rate", 300), ("voice", "silence"), ("voice", None), ("engine", "fake")],
)
def test_set_then_get(client, key, value):
    r = client.request({"type": "set", "key": key, "value": value})
    assert r["ok"] is True, r
    assert r["key"] == key
    assert r["value"] == value
    r = client.request({"type": "get", "key": key})
    assert r["value"] == value


@pytest.mark.parametrize(
    "key,value,code",
    [
        ("volume", 101, "E_BAD_REQUEST"),
        ("volume", "loud", "E_BAD_REQUEST"),
        ("rate", 50, "E_BAD_REQUEST"),
        ("rate", 1.5, "E_BAD_REQUEST"),
        ("voice", "nobody", "E_NOT_FOUND"),
        ("engine", "nothing", "E_NOT_FOUND"),
        ("colour", 1, "E_BAD_REQUEST"),
        ("audio_mode", "duck", "E_UNSUPPORTED"),
    ],
)
def test_bad_settings(client, key, value, code):
    r = client.request({"type": "set", "key": key, "value": value})
    assert r["ok"] is False
    assert r["error"]["code"] == code


def test_settings_show_in_state(client):
    client.request({"type": "subscribe", "events": ["state"]})
    client.state()
    client.request({"type": "set", "key": "volume", "value": 55})
    client.state(lambda s: s["volume"] == 55)
    client.request({"type": "set", "key": "rate", "value": 250})
    s = client.state(lambda s: s["rate"] == 250)
    assert s["engine_status"]["engine"] == "fake"


def test_engine_status_says_the_engine_is_ready(client, rt):
    # Protocol 1.1: readiness in every state event. The fake engine is
    # always ready, so there is no progress, fallback or message.
    assert "engine_status" in rt.info["capabilities"]
    client.request({"type": "subscribe", "events": ["state"]})
    s = client.state()
    assert s["engine_status"] == {"engine": "fake", "ready": True, "status": "ready"}


def test_voices(client):
    r = client.request({"type": "voices"})
    assert r["ok"] is True
    v = r["voices"][0]
    assert set(v) >= {"id", "name", "language", "engine", "license_class", "installed"}
    assert v["engine"] == "fake"
    assert v["license_class"] in ("permissive", "os")
    assert client.request({"type": "voices", "engine": "fake"})["voices"] == r["voices"]
    assert client.request({"type": "voices", "engine": "nope"})["error"]["code"] == "E_NOT_FOUND"


def test_state_events_have_every_field_and_increasing_seq(client):
    client.request({"type": "subscribe", "events": ["state"]})
    first = client.state()
    assert set(first) >= {
        "event", "seq", "now_playing", "queued", "paused", "muted",
        "volume", "rate", "voice", "engine_status",
    }
    client.request({"type": "speak", "text": "One is here. Two is here."})
    seqs = [first["seq"]]
    while True:
        s = client.state()
        seqs.append(s["seq"])
        if s["now_playing"] is None:
            break
    assert seqs == sorted(seqs) and len(set(seqs)) == len(seqs)
    assert not any(e.get("event") == "item" for e in client.events), "items not subscribed"


def test_a_failed_chunk_is_logged_and_the_item_fails(client):
    client.request({"type": "subscribe", "events": ["items", "log"]})
    item = client.request({"type": "speak", "text": "This [fail] breaks."})["item_id"]
    log = client.next_event(lambda e: e.get("event") == "log")
    assert log["message"]
    client.item(item, "failed")


def test_subscribe_with_an_unknown_stream(client):
    r = client.request({"type": "subscribe", "events": ["state", "weather"]})
    assert r["error"]["code"] == "E_UNSUPPORTED"


def test_sse_carries_state_and_item_events(rt):
    sse = rt.sse("state,items")
    assert sse.status == 200
    assert sse.content_type.startswith("text/event-stream")
    name, first = sse.next_event()
    assert name == "state" and first["now_playing"] is None
    _, r = rt.post("speak", {"text": SHORT_SENTENCE})
    item = r["item_id"]
    name, e = sse.next_event(lambda n, e: n == "item" and e["phase"] == "started")
    assert e == {"event": "item", "item_id": item, "phase": "started"}
    sse.next_event(lambda n, e: n == "state" and e["now_playing"] is not None)
    sse.next_event(lambda n, e: n == "item" and e["phase"] == "finished")
    sse.close()


def test_sse_with_an_unknown_stream_is_rejected(rt):
    import urllib.error

    with pytest.raises(urllib.error.HTTPError) as e:
        rt.sse("weather")
    assert e.value.code == 400


def test_subscribe_over_post_is_a_bad_request(rt):
    status, r = rt.post("subscribe", {"events": ["state"]})
    assert status == 400
    assert r["error"]["code"] == "E_BAD_REQUEST"


def test_unknown_type(client, rt):
    r = client.request({"type": "dance", "id": 5})
    assert r == {"id": 5, "ok": False, "error": {"code": "E_UNKNOWN_TYPE", "message": r["error"]["message"]}}
    status, r = rt.post("dance", {})
    assert status == 404
    assert r["error"]["code"] == "E_UNKNOWN_TYPE"


def test_extension_messages_are_unsupported(client):
    for msg in (
        {"type": "channel_open", "channel": "a"},
        {"type": "turn_start", "channel": "a", "turn": 1},
        {"type": "control", "action": "next_channel"},
        {"type": "get", "key": "settings_url"},
    ):
        r = client.request(msg)
        assert r["error"]["code"] == "E_UNSUPPORTED", msg


@pytest.mark.parametrize(
    "msg",
    [
        {"type": "speak"},
        {"type": "speak", "text": 5},
        {"type": "speak", "text": "a", "mode": "sideways"},
        {"type": "speak", "text": "a", "interrupt": "yes"},
        {"type": "control"},
        {"type": "control", "action": "fly"},
        {"type": "get"},
        {"no_type": True},
    ],
)
def test_bad_requests(client, msg):
    r = client.request(msg)
    assert r["error"]["code"] == "E_BAD_REQUEST", r


def test_invalid_json_after_hello_keeps_the_connection(client):
    client.send_raw(b"{not json\n")
    r = client.reply()
    assert r["error"]["code"] == "E_BAD_REQUEST"
    assert client.request({"type": "get", "key": "rate"})["ok"] is True


def test_http_body_must_be_an_object(rt):
    status, r = rt.post("speak", raw=b"[1, 2]")
    assert status == 400
    assert r["error"]["code"] == "E_BAD_REQUEST"


def test_unknown_fields_are_ignored(client):
    r = client.request({"type": "speak", "text": long_text(1), "channel": "x", "future": [1, {"a": 2}]})
    assert r["ok"] is True
    r = client.request({"type": "control", "action": "stop", "why": "test"})
    assert r["ok"] is True
    r = client.request({"type": "get", "key": "volume", "extra": None})
    assert r["ok"] is True
