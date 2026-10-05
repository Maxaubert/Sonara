"""Extension ``channels`` (spec 4.2): named channels with a policy, focus,
next_channel, close, switch announcements and extension gating, observed
through state and item events. Ported from the Python plugin's router,
channel and multi-session daemon tests (the parts that are not agent
features)."""
from __future__ import annotations

import time

from harness import SHORT_SENTENCE, long_text

LONG = long_text(1)


def channels_client(rt):
    """A TCP client with the extension enabled, subscribed to state and
    items, past the first (idle) state."""
    c = rt.tcp(extensions=["channels"])
    assert c.request({"type": "subscribe", "events": ["state", "items"]})["ok"]
    c.state()
    return c


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def open_two(c, policy="queue"):
    ok(c, {"type": "channel_open", "channel": "a", "label": "Alpha", "host_tab": "tab-a", "policy": policy})
    ok(c, {"type": "channel_open", "channel": "b", "label": "Beta", "policy": policy})


def heard(c, count):
    """The next ``count`` items to start, as (first chunk text, channel),
    read from the state events in order (an item counts once, across
    calls)."""
    out, last = [], getattr(c, "last_item", None)
    while len(out) < count:
        e = c.next_event()
        if e.get("event") != "state" or e["now_playing"] is None:
            continue
        np = e["now_playing"]
        if np["item_id"] != last:
            last = np["item_id"]
            out.append((np["text"], np.get("channel")))
    c.last_item = last
    return out


def quiet(c, seconds=0.6):
    """Nothing starts for ``seconds``."""
    end = time.monotonic() + seconds
    while True:
        left = end - time.monotonic()
        if left <= 0:
            return
        try:
            e = c.next_event(timeout=left)
        except AssertionError:
            return
        if e.get("event") == "state":
            assert e["now_playing"] is None, e


def is_long(heard_item, channel):
    """``heard_item`` is the start of LONG in ``channel``."""
    text, ch = heard_item
    return text.startswith("Sentence 0") and ch == channel


def test_hello_enables_channels_for_the_instance(rt):
    assert "channels" in rt.read_runtime()["extensions"]
    plain = rt.tcp()
    r = plain.request({"type": "channel_open", "channel": "a"})
    assert r["error"]["code"] == "E_UNSUPPORTED"
    r = plain.request({"type": "control", "action": "next_channel"})
    assert r["error"]["code"] == "E_UNSUPPORTED"
    r = plain.request({"type": "control", "action": "flush"})
    assert r["error"]["code"] == "E_UNSUPPORTED"
    r = plain.request({"type": "get", "key": "channel_announce"})
    assert r["error"]["code"] == "E_UNSUPPORTED"
    c = rt.tcp(hello=False)
    r = c.hello(rt.token, extensions=["channels"])
    assert r["extensions"] == ["channels"]
    assert r["unavailable"] == []
    # Enabled for every client of the instance from now on.
    ok(plain, {"type": "channel_open", "channel": "a"})


def test_channel_open_replies_and_reopen_updates(rt):
    c = channels_client(rt)
    r = ok(c, {"type": "channel_open", "channel": "a", "label": "Alpha"})
    assert r["created"] is True and r["policy"] == "latest"
    r = ok(c, {"type": "channel_open", "channel": "a", "label": "Renamed", "policy": "queue"})
    assert r["created"] is False and r["policy"] == "queue"
    for msg in (
        {"type": "channel_open", "channel": ""},
        {"type": "channel_open", "channel": "x", "policy": "newest"},
    ):
        assert c.request(msg)["error"]["code"] == "E_BAD_REQUEST"
    for msg in (
        {"type": "channel_close", "channel": "nope"},
        {"type": "focus", "channel": "nope"},
    ):
        assert c.request(msg)["error"]["code"] == "E_NOT_FOUND"


def test_latest_policy_reads_only_the_newest_message(rt):
    c = channels_client(rt)
    ok(c, {"type": "channel_open", "channel": "a", "policy": "latest"})
    first = ok(c, {"type": "speak", "channel": "a", "text": LONG})
    assert first["item_id"] is not None
    assert ok(c, {"type": "speak", "channel": "a", "text": "Middle one."})["item_id"] is None
    r = ok(c, {"type": "speak", "channel": "a", "text": "Newest one."})
    assert r["dropped"] == 1
    assert is_long(heard(c, 1)[0], "a")
    ok(c, {"type": "control", "action": "skip"})
    assert heard(c, 1) == [("Newest one.", "a")]
    quiet(c)


def test_queue_policy_reads_every_message_in_order(rt):
    c = channels_client(rt)
    ok(c, {"type": "channel_open", "channel": "a", "policy": "queue"})
    ok(c, {"type": "speak", "channel": "a", "text": LONG})
    ok(c, {"type": "speak", "channel": "a", "text": "Second one."})
    ok(c, {"type": "speak", "channel": "a", "text": "Third one."})
    ok(c, {"type": "control", "action": "pause"})
    s = c.state(lambda s: s["paused"] is True)
    assert s["queued"] == 2, "a channel's unread messages count as queued"
    ok(c, {"type": "control", "action": "play"})
    heard(c, 1)
    ok(c, {"type": "control", "action": "skip"})
    assert heard(c, 2) == [("Second one.", "a"), ("Third one.", "a")]


def test_a_switch_is_announced_and_state_names_the_channel(rt):
    c = channels_client(rt)
    open_two(c)
    ok(c, {"type": "speak", "channel": "a", "text": "From alpha."})
    ok(c, {"type": "speak", "channel": "b", "text": "From beta."})
    assert heard(c, 3) == [("From alpha.", "a"), ("Beta.", "b"), ("From beta.", "b")]
    ok(c, {"type": "speak", "channel": "a", "text": "Alpha again."})
    s = c.state(lambda s: s["now_playing"] is not None and s["now_playing"]["text"] == "Alpha again.")
    assert s["now_playing"]["host_tab"] == "tab-a"


def test_announcements_can_be_turned_off(rt):
    c = channels_client(rt)
    open_two(c)
    r = ok(c, {"type": "set", "key": "channel_announce", "value": "off"})
    assert r["value"] == "off"
    assert ok(c, {"type": "get", "key": "channel_announce"})["value"] == "off"
    ok(c, {"type": "speak", "channel": "a", "text": "From alpha."})
    ok(c, {"type": "speak", "channel": "b", "text": "From beta."})
    assert heard(c, 2) == [("From alpha.", "a"), ("From beta.", "b")]


def test_the_reading_channel_finishes_before_the_focused_one(rt):
    c = channels_client(rt)
    open_two(c)
    ok(c, {"type": "channel_open", "channel": "c", "label": "Gamma", "policy": "queue"})
    ok(c, {"type": "speak", "channel": "a", "text": LONG})
    ok(c, {"type": "speak", "channel": "a", "text": "More alpha."})
    ok(c, {"type": "speak", "channel": "b", "text": "From beta."})
    ok(c, {"type": "speak", "channel": "c", "text": "From gamma."})
    ok(c, {"type": "focus", "channel": "c"})
    heard(c, 1)
    ok(c, {"type": "control", "action": "skip"})
    assert heard(c, 5) == [
        ("More alpha.", "a"),
        ("Gamma.", "c"),
        ("From gamma.", "c"),
        ("Beta.", "b"),
        ("From beta.", "b"),
    ]


def test_next_channel_switches_now_and_does_not_auto_resume(rt):
    c = channels_client(rt)
    open_two(c)
    ok(c, {"type": "speak", "channel": "a", "text": LONG})
    ok(c, {"type": "speak", "channel": "b", "text": "From beta."})
    heard(c, 1)
    r = ok(c, {"type": "control", "action": "next_channel"})
    assert r["channel"] == "b"
    assert heard(c, 2) == [("Beta.", "b"), ("From beta.", "b")]
    quiet(c)  # alpha was left on purpose
    r = ok(c, {"type": "control", "action": "next_channel"})
    assert r["channel"] == "a"
    # Its cut message was never heard to the end: it resumes.
    h = heard(c, 2)
    assert h[0] == ("Alpha.", "a") and is_long(h[1], "a")


def test_next_channel_replays_a_heard_channel(rt):
    c = channels_client(rt)
    open_two(c)
    ok(c, {"type": "speak", "channel": "a", "text": "From alpha."})
    ok(c, {"type": "speak", "channel": "b", "text": "From beta."})
    assert len(heard(c, 3)) == 3
    quiet(c, 0.3)
    assert ok(c, {"type": "control", "action": "next_channel"})["channel"] == "a"
    assert heard(c, 2) == [("Alpha, reading again.", "a"), ("From alpha.", "a")]


def test_closing_the_reading_channel_cuts_it(rt):
    c = channels_client(rt)
    open_two(c)
    a = ok(c, {"type": "speak", "channel": "a", "text": LONG})["item_id"]
    ok(c, {"type": "speak", "channel": "b", "text": "From beta."})
    heard(c, 1)
    ok(c, {"type": "channel_close", "channel": "a"})
    c.item(a, "skipped")
    # The user heard the closed channel last: the switch is announced (#241).
    assert heard(c, 2) == [("Beta.", "b"), ("From beta.", "b")]


def test_stop_flushes_every_channel_and_restart_replays(rt):
    c = channels_client(rt)
    open_two(c)
    ok(c, {"type": "speak", "channel": "a", "text": LONG})
    ok(c, {"type": "speak", "channel": "a", "text": "More alpha."})
    ok(c, {"type": "speak", "channel": "b", "text": "From beta."})
    heard(c, 1)
    ok(c, {"type": "control", "action": "stop"})
    c.state(lambda s: s["now_playing"] is None and s["queued"] == 0)
    quiet(c)
    ok(c, {"type": "control", "action": "restart"})
    h = heard(c, 2)
    assert is_long(h[0], "a") and h[1] == ("More alpha.", "a")


def test_stop_with_a_channel_flushes_only_it(rt):
    c = channels_client(rt)
    open_two(c)
    ok(c, {"type": "speak", "channel": "a", "text": LONG})
    ok(c, {"type": "speak", "channel": "a", "text": "More alpha."})
    ok(c, {"type": "speak", "channel": "b", "text": "From beta."})
    ok(c, {"type": "control", "action": "stop", "channel": "b"})
    assert is_long(heard(c, 1)[0], "a")
    ok(c, {"type": "control", "action": "skip"})
    assert heard(c, 1) == [("More alpha.", "a")]
    quiet(c)


def test_flush_stops_only_the_channel_being_read(rt):
    # #228: the flush hotkey's action. The channel being read is skipped to
    # its end; the other channels are read next.
    c = channels_client(rt)
    open_two(c)
    ok(c, {"type": "set", "key": "channel_announce", "value": "off"})
    ok(c, {"type": "speak", "channel": "a", "text": LONG})
    ok(c, {"type": "speak", "channel": "a", "text": "More alpha."})
    ok(c, {"type": "speak", "channel": "b", "text": "From beta."})
    assert is_long(heard(c, 1)[0], "a")
    r = ok(c, {"type": "control", "action": "flush"})
    assert (r["flushed"], r["channel"]) == ("channel", "a")
    assert heard(c, 1) == [("From beta.", "b")]
    quiet(c)
    r = ok(c, {"type": "control", "action": "flush"})
    assert (r["flushed"], r["channel"]) == ("nothing", None)
    r = c.request({"type": "control", "action": "flush", "channel": "a"})
    assert r["error"]["code"] == "E_BAD_REQUEST"


def test_interrupt_reads_the_new_message_now(rt):
    c = channels_client(rt)
    open_two(c)
    a = ok(c, {"type": "speak", "channel": "a", "text": LONG})["item_id"]
    heard(c, 1)
    r = ok(c, {"type": "speak", "channel": "b", "text": "Urgent beta.", "interrupt": True})
    assert r["item_id"] is None  # the announcement goes first
    c.item(a, "skipped")
    assert heard(c, 2) == [("Beta.", "b"), ("Urgent beta.", "b")]


def test_a_message_cut_by_another_channels_interrupt_is_read_again(rt):
    c = channels_client(rt)
    open_two(c, policy="latest")
    a = ok(c, {"type": "speak", "channel": "a", "text": LONG})["item_id"]
    heard(c, 1)
    ok(c, {"type": "speak", "channel": "b", "text": "Urgent beta.", "interrupt": True})
    c.item(a, "skipped")
    assert heard(c, 2) == [("Beta.", "b"), ("Urgent beta.", "b")]
    h = heard(c, 2)
    assert h[0] == ("Alpha.", "a") and is_long(h[1], "a")


def test_text_without_a_channel_is_core_and_read_first(rt):
    c = channels_client(rt)
    open_two(c)
    core = ok(c, {"type": "speak", "text": LONG})
    assert core["item_id"] is not None
    assert ok(c, {"type": "speak", "channel": "a", "text": "From alpha."})["item_id"] is None
    assert is_long(heard(c, 1)[0], None)
    ok(c, {"type": "control", "action": "skip"})
    assert heard(c, 1) == [("From alpha.", "a")]


def test_channels_over_http(rt):
    status, r = rt.post("hello", {"extensions": ["channels"]})
    assert status == 200 and r["extensions"] == ["channels"]
    status, r = rt.post("channel_open", {"channel": "h", "label": "Http"})
    assert status == 200 and r["created"] is True
    status, r = rt.post("speak", {"channel": "h", "text": SHORT_SENTENCE})
    assert status == 200 and r["channel"] == "h"
    status, r = rt.post("control", {"action": "next_channel"})
    assert status == 200 and r["channel"] == "h"
    status, r = rt.post("focus", {"channel": "missing"})
    assert status == 404 and r["error"]["code"] == "E_NOT_FOUND"


def test_keep_label_keeps_the_label_an_open_channel_has(rt):
    """#245: ``keep_label`` (the Claude hook's) never renames a channel that
    has a label; without it a host still renames its channel."""
    c = channels_client(rt)
    for label in ("Filesmith", "statusbar", "Filesmith", "statusbar"):
        ok(c, {"type": "channel_open", "channel": "f", "label": label, "keep_label": True})
    ok(c, {"type": "speak", "channel": "f", "text": SHORT_SENTENCE})
    s = c.state(lambda s: s["now_playing"] is not None)
    assert s["now_playing"]["label"] == "Filesmith"
    c.state(lambda s: s["now_playing"] is None)
    ok(c, {"type": "channel_open", "channel": "f", "label": "Renamed"})
    ok(c, {"type": "speak", "channel": "f", "text": SHORT_SENTENCE})
    s = c.state(lambda s: s["now_playing"] is not None)
    assert s["now_playing"]["label"] == "Renamed"
    r = c.request({"type": "channel_open", "channel": "f", "keep_label": "yes"})
    assert r["error"]["code"] == "E_BAD_REQUEST"
