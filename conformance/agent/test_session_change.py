"""Extension ``agent``: session switches and custom earcons (#209). With the
agent on, a switch the user hears plays the ``session_change`` earcon and
then says "Session changed: <label>." (a replay: "..., reading again."), as
the Python plugin did, for automatic hand-offs and for ``control
next_channel``. ``<home>/earcons/<kind>.wav`` replaces a bundled earcon
(``get earcons`` lists it). The runtime logs its start in
``logs/sonarad.log``. Hotkey presses are covered by
``crates/sonarad/tests/system.rs`` (the fake platform presses keys there)."""
from __future__ import annotations

import struct

from test_agent import agent_client, earcon, heard, no_earcon, ok, stream


def two_sessions(c):
    for ch, label in (("a", "alpha-repo"), ("b", "beta-repo")):
        ok(c, {"type": "channel_open", "channel": ch, "label": label})


def test_an_automatic_hand_off_chimes_and_says_session_changed(rt):
    c = agent_client(rt, announce=True, policy="all")
    two_sessions(c)
    stream(c, "a", "Alpha words.")
    stream(c, "b", "Beta words.")
    assert heard(c, 3) == [
        ("Alpha words.", "a"),
        ("Session changed: beta-repo.", "b"),
        ("Beta words.", "b"),
    ]
    assert earcon(c) == "session_change"
    no_earcon(c)


def test_next_channel_chimes_and_says_session_changed_reading_again(rt):
    c = agent_client(rt, announce=True, policy="all")
    two_sessions(c)
    stream(c, "a", "Alpha words.")
    assert heard(c, 1) == [("Alpha words.", "a")]
    c.state(lambda s: s["now_playing"] is None)
    r = ok(c, {"type": "control", "action": "next_channel"})
    assert r["channel"] == "a"
    assert heard(c, 2) == [
        ("Session changed: alpha-repo, reading again.", "a"),
        ("Alpha words.", "a"),
    ]
    assert earcon(c) == "session_change"
    no_earcon(c)


def test_a_question_after_the_last_reader_closed_is_announced(rt):
    """#241, the log of 2026-10-04 23:29:55: "agent-hooks" read its reply and
    its session ended; a question in "work" was then read without "Session
    changed"."""
    c = agent_client(rt, announce=True, policy="all")
    ok(c, {"type": "channel_open", "channel": "h", "label": "agent-hooks"})
    ok(c, {"type": "focus", "channel": "h"})
    ok(c, {"type": "turn_start", "channel": "h"})
    stream(c, "h", "ok")
    ok(c, {"type": "turn_end", "channel": "h"})
    assert heard(c, 1) == [("ok", "h")]
    assert earcon(c) == "turn_done"
    c.state(lambda s: s["now_playing"] is None)
    ok(c, {"type": "channel_close", "channel": "h"})
    ok(c, {"type": "channel_open", "channel": "w", "label": "work"})
    ok(c, {"type": "focus", "channel": "w"})
    ok(c, {"type": "turn_start", "channel": "w"})
    ok(c, {"type": "ask", "channel": "w", "kind": "question", "text": "Red or blue?",
           "options": ["Red", "Blue"], "label": "work"})
    assert heard(c, 2) == [
        ("Session changed: work.", "w"),
        ("Red or blue?", "w"),
    ]
    assert sorted([earcon(c), earcon(c)]) == ["choice", "session_change"]
    no_earcon(c)


def test_a_session_first_seen_by_its_stream_is_announced_with_its_label(rt):
    """#241: after a restart a session's first message is often a stream;
    the label it carries names the session."""
    c = agent_client(rt, announce=True, policy="all")
    ok(c, {"type": "channel_open", "channel": "a", "label": "alpha-repo"})
    stream(c, "a", "Alpha words.")
    ok(c, {"type": "stream", "channel": "b", "delta": "Beta words.", "final": True,
           "label": "beta-repo"})
    assert heard(c, 3) == [
        ("Alpha words.", "a"),
        ("Session changed: beta-repo.", "b"),
        ("Beta words.", "b"),
    ]


def test_a_session_without_a_label_is_announced_without_a_name(rt):
    c = agent_client(rt, announce=True, policy="all")
    ok(c, {"type": "channel_open", "channel": "a", "label": "alpha-repo"})
    stream(c, "a", "Alpha words.")
    stream(c, "b", "Beta words.")
    assert heard(c, 3) == [
        ("Alpha words.", "a"),
        ("Session changed.", "b"),
        ("Beta words.", "b"),
    ]
    assert earcon(c) == "session_change"


def test_announcements_off_say_nothing_and_do_not_chime(rt):
    c = agent_client(rt, announce=False, policy="all")
    two_sessions(c)
    stream(c, "a", "Alpha words.")
    stream(c, "b", "Beta words.")
    assert heard(c, 2) == [("Alpha words.", "a"), ("Beta words.", "b")]
    no_earcon(c)


def test_channels_without_the_agent_keep_the_generic_announcement(rt):
    c = rt.tcp(extensions=["channels"])
    assert c.request({"type": "subscribe", "events": ["state"]})["ok"]
    c.state()
    two_sessions(c)
    for ch, text in (("a", "Alpha words."), ("b", "Beta words.")):
        ok(c, {"type": "speak", "channel": ch, "text": text})
    assert heard(c, 3) == [
        ("Alpha words.", "a"),
        ("beta-repo.", "b"),
        ("Beta words.", "b"),
    ]


def wav(rate: int, frames: int, bits: int = 24, channels: int = 2) -> bytes:
    """A square-wave WAV (integer PCM)."""
    width = bits // 8
    data = bytearray()
    for i in range(frames):
        v = (1 << (bits - 2)) * (1 if (i // 20) % 2 == 0 else -1)
        data += v.to_bytes(width, "little", signed=True) * channels
    fmt = struct.pack("<HHIIHH", 1, channels, rate, rate * width * channels, width * channels, bits)
    body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt
    body += b"data" + struct.pack("<I", len(data)) + bytes(data)
    return b"RIFF" + struct.pack("<I", len(body)) + body


def test_custom_earcons_folder(rt):
    c = agent_client(rt)
    v = ok(c, {"type": "get", "key": "earcons"})["value"]
    folder = rt.home / "earcons"
    assert v["folder"] == str(folder)
    assert folder.is_dir(), "created at startup so the user can drop WAVs in"
    assert v["custom"] == []
    assert sorted(v["kinds"]) == sorted([
        "choice", "permission", "error", "turn_done", "nav", "nav_edge",
        "session_change", "summary_failed",
    ])
    (folder / "session_change.wav").write_bytes(wav(48000, 4800))
    (folder / "turn_done.wav").write_bytes(b"RIFF junk")
    v = ok(c, {"type": "get", "key": "earcons"})["value"]
    assert v["custom"] == ["session_change"]
    r = c.request({"type": "set", "key": "earcons", "value": {}})
    assert r["error"]["code"] == "E_BAD_REQUEST"
    # A bad file plays the bundled clip and is logged once.
    ok(c, {"type": "earcon", "kind": "turn_done"})
    assert earcon(c) == "turn_done"
    log = (rt.home / "logs" / "sonarad.log").read_text(encoding="utf-8")
    assert log.count("turn_done.wav") == 1, log
    assert "using the bundled turn_done clip" in log


def test_the_start_is_logged(rt):
    log = (rt.home / "logs" / "sonarad.log").read_text(encoding="utf-8")
    assert "started (pid" in log and "engine fake ready" in log, log
    assert rt.read_runtime()["version"] in log


def test_label_is_checked_only_on_messages_that_name_a_channel(rt):
    """#241: ``label`` names the message's channel. ``earcon`` names none, so
    a label there is ignored as before; on a channel message it must be a
    string."""
    c = agent_client(rt, announce=True, policy="all")
    ok(c, {"type": "earcon", "kind": "nav", "label": 5})
    r = c.request({"type": "stream", "channel": "a", "delta": "x", "label": 5})
    assert r["error"]["code"] == "E_BAD_REQUEST"
