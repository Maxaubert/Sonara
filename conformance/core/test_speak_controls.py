"""Core: speak modes and every control (spec 4.1), observed through
state and item events."""
from __future__ import annotations

from harness import SHORT_SENTENCE, long_text


def subscribed(c):
    r = c.request({"type": "subscribe", "events": ["state", "items"]})
    assert r["ok"] is True
    first = c.state()
    assert first["now_playing"] is None
    return c


def playing(c, item_id, chunk=0):
    return c.state(
        lambda s: s["now_playing"] is not None
        and s["now_playing"]["item_id"] == item_id
        and s["now_playing"]["chunk"] == chunk
    )


def test_speak_returns_increasing_item_ids(client):
    a = client.request({"type": "speak", "text": SHORT_SENTENCE})
    b = client.request({"type": "speak", "text": SHORT_SENTENCE})
    assert a["ok"] and b["ok"]
    assert b["item_id"] > a["item_id"] >= 1


def test_an_item_starts_and_finishes(client):
    subscribed(client)
    r = client.request({"type": "speak", "text": "One is here. Two is here.", "label": "greeting"})
    item = r["item_id"]
    client.item(item, "started")
    s = playing(client, item)
    assert s["now_playing"]["label"] == "greeting"
    assert s["now_playing"]["chunks"] == 2
    assert s["now_playing"]["text"] == "One is here."
    client.item(item, "finished")
    client.state(lambda s: s["now_playing"] is None)


def test_unspeakable_text_is_skipped(client):
    subscribed(client)
    r = client.request({"type": "speak", "text": ""})
    client.item(r["item_id"], "skipped")


def test_append_queues_after_the_current_item(client):
    subscribed(client)
    a = client.request({"type": "speak", "text": long_text(2)})["item_id"]
    b = client.request({"type": "speak", "text": long_text(1), "mode": "append"})["item_id"]
    client.state(lambda s: s["queued"] == 1)
    client.request({"type": "control", "action": "skip"})
    client.item(a, "skipped")
    client.item(b, "started")


def test_replace_drops_unread_items_but_not_the_current_one(client):
    subscribed(client)
    a = client.request({"type": "speak", "text": long_text(2)})["item_id"]
    b = client.request({"type": "speak", "text": long_text(1)})["item_id"]
    c = client.request({"type": "speak", "text": long_text(1), "mode": "replace"})["item_id"]
    client.item(b, "skipped")
    s = client.state(lambda s: s["queued"] == 1)
    assert s["now_playing"]["item_id"] == a
    client.request({"type": "control", "action": "skip"})
    client.item(c, "started")


def test_interrupt_cuts_the_current_item(client):
    subscribed(client)
    a = client.request({"type": "speak", "text": long_text(2)})["item_id"]
    client.item(a, "started")
    b = client.request({"type": "speak", "text": long_text(1), "interrupt": True})["item_id"]
    client.item(a, "skipped")
    client.item(b, "started")
    playing(client, b)


def test_pause_play_and_toggle(client):
    subscribed(client)
    a = client.request({"type": "speak", "text": long_text(2)})["item_id"]
    playing(client, a)
    assert client.request({"type": "control", "action": "pause"})["ok"]
    client.state(lambda s: s["paused"] is True)
    client.request({"type": "control", "action": "play"})
    client.state(lambda s: s["paused"] is False)
    client.request({"type": "control", "action": "toggle"})
    client.state(lambda s: s["paused"] is True)
    client.request({"type": "control", "action": "toggle"})
    client.state(lambda s: s["paused"] is False)


def test_next_previous_and_restart_move_between_chunks(client):
    subscribed(client)
    a = client.request({"type": "speak", "text": long_text(3)})["item_id"]
    playing(client, a, 0)
    client.request({"type": "control", "action": "next"})
    playing(client, a, 1)
    client.request({"type": "control", "action": "next"})
    playing(client, a, 2)
    client.request({"type": "control", "action": "previous"})
    playing(client, a, 1)
    client.request({"type": "control", "action": "restart"})
    playing(client, a, 0)


def test_skip_ends_the_item(client):
    subscribed(client)
    a = client.request({"type": "speak", "text": long_text(3)})["item_id"]
    client.item(a, "started")
    client.request({"type": "control", "action": "skip"})
    client.item(a, "skipped")
    client.state(lambda s: s["now_playing"] is None)


def test_stop_clears_everything(client):
    subscribed(client)
    a = client.request({"type": "speak", "text": long_text(2)})["item_id"]
    b = client.request({"type": "speak", "text": long_text(2)})["item_id"]
    client.request({"type": "control", "action": "stop"})
    client.item(a, "skipped")
    client.item(b, "skipped")
    client.state(lambda s: s["now_playing"] is None and s["queued"] == 0)


def test_mute_and_unmute(client):
    subscribed(client)
    client.request({"type": "control", "action": "mute"})
    client.state(lambda s: s["muted"] is True)
    client.request({"type": "control", "action": "unmute"})
    client.state(lambda s: s["muted"] is False)


def test_rapid_controls_leave_a_consistent_state(client):
    subscribed(client)
    a = client.request({"type": "speak", "text": long_text(3)})["item_id"]
    playing(client, a)
    for action in ["pause", "play", "next", "previous", "restart", "toggle"] * 5:
        assert client.request({"type": "control", "action": action})["ok"]
    client.request({"type": "control", "action": "play"})
    s = client.state(lambda s: s["paused"] is False and s["now_playing"] is not None)
    assert s["now_playing"]["item_id"] == a


def test_speak_and_control_over_http(rt):
    status, r = rt.post("speak", {"text": long_text(1)})
    assert status == 200 and r["ok"] and r["item_id"] >= 1
    status, r = rt.post("control", {"action": "stop", "id": 9})
    assert status == 200
    assert r == {"ok": True, "id": 9}
