"""Persisted settings (#201): every setting a client sets survives a restart
of the runtime on the same home, ``config.json`` holds only the keys that
were set, and per-channel preferences live in ``session_prefs.json``.

Each test starts a runtime, changes settings over the protocol, ends it
(killed, so nothing depends on a clean exit) and starts another one on the
same home."""
from __future__ import annotations

import json

from harness import long_text


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def get(c, key):
    return ok(c, {"type": "get", "key": key})["value"]


def set_key(c, key, value):
    return ok(c, {"type": "set", "key": key, "value": value})["value"]


def saved(rt) -> dict:
    p = rt.home / "config.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else {}


def restart(start, rt):
    rt.close()
    return start(home=rt.home)


ALL = ["agent", "system"]


def test_a_fresh_home_writes_no_config(rt):
    c = rt.tcp(extensions=ALL)
    assert get(c, "rate") == 200
    assert get(c, "summaries")["enabled"] is False
    assert not (rt.home / "config.json").exists()


def test_every_setting_survives_a_restart(start):
    rt = start()
    c = rt.tcp(extensions=ALL)
    changes = {
        "volume": 40,
        "rate": 260,
        "voice": "silence",
        "channel_announce": "off",
        "mute_level": 1,
        "verbosity": "medium",
        "minqueue": 4,
        "audio_mode": "pause",
        "duck_level": 15,
    }
    for key, value in changes.items():
        assert set_key(c, key, value) == value, key
    s = set_key(c, "summaries", {"enabled": True, "command": "codex", "model": "gpt-5.4-mini",
                                 "timeout": 90, "settle_ms": 300, "style": "brief",
                                 "prompts": {"brief": "One line.", "tidy": "All of it."}})
    assert s["prompt"] == "One line."
    c.close()

    rt = restart(start, rt)
    c = rt.tcp(extensions=ALL)
    for key, value in changes.items():
        assert get(c, key) == value, key
    s = get(c, "summaries")
    assert {k: s[k] for k in ("enabled", "command", "model", "timeout", "settle_ms", "style")} == {
        "enabled": True, "command": "codex", "model": "gpt-5.4-mini",
        "timeout": 90, "settle_ms": 300, "style": "brief"}
    assert s["prompts"] == {"brief": "One line.", "tidy": "All of it."}
    assert s["prompt"] == "One line."


def test_core_settings_apply_before_the_first_speech(start):
    """The reader starts with the saved rate and volume: the first state
    event of an item already carries them, without any client setting them."""
    rt = start()
    c = rt.tcp()
    set_key(c, "rate", 300)
    set_key(c, "volume", 55)
    c.close()
    rt = restart(start, rt)
    c = rt.tcp()
    ok(c, {"type": "subscribe", "events": ["state"]})
    ok(c, {"type": "speak", "text": long_text(1)})
    s = c.state(lambda s: s["now_playing"] is not None)
    assert (s["rate"], s["volume"]) == (300, 55)


def test_only_the_keys_that_were_set_are_saved(rt):
    c = rt.tcp(extensions=ALL)
    set_key(c, "rate", 250)
    set_key(c, "summaries", {"style": "tidy"})
    r = c.request({"type": "set", "key": "volume", "value": 101})
    assert r["error"]["code"] == "E_BAD_REQUEST"
    assert saved(rt) == {"rate": 250, "summaries": {"style": "tidy"}}


def test_a_corrupt_config_gives_the_defaults(start, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    (home / "config.json").write_text('{"rate": 9999, "volume": 30, ', encoding="utf-8")
    rt = start(home=home)
    c = rt.tcp()
    assert get(c, "rate") == 200
    assert get(c, "volume") == 100
    (home / "config.json").write_text('{"rate": 9999, "volume": 30}', encoding="utf-8")
    rt = restart(start, rt)
    c = rt.tcp()
    assert get(c, "rate") == 200, "an out-of-range value is dropped"
    assert get(c, "volume") == 30, "the others still apply"
    log = (home / "logs" / "sonarad.log").read_text(encoding="utf-8")
    assert "rate" in log


def test_channel_prefs_survive_a_restart_and_rename_the_channel(start):
    rt = start()
    c = rt.tcp(extensions=ALL)
    ok(c, {"type": "channel_open", "channel": "s1", "label": "repo"})
    rows = set_key(c, "channel_prefs", {"channel": "s1", "label": "Build", "voice": "silence",
                                        "muted": True})
    assert rows[0] == {"channel": "s1", "open": True, "reading": False, "client_label": "repo",
                       "host_tab": None, "label": "Build", "voice": "silence", "muted": True}
    c.close()
    rt = restart(start, rt)
    c = rt.tcp(extensions=ALL)
    rows = get(c, "channel_prefs")
    assert [(r["channel"], r["open"], r["label"], r["muted"]) for r in rows] == [
        ("s1", False, "Build", True)]
    ok(c, {"type": "set", "key": "channel_announce", "value": "on"})
    ok(c, {"type": "subscribe", "events": ["state"]})
    ok(c, {"type": "channel_open", "channel": "s1", "label": "repo"})
    ok(c, {"type": "channel_open", "channel": "s0", "label": "other"})
    ok(c, {"type": "speak", "channel": "s1", "text": "Hello."})
    ok(c, {"type": "control", "action": "next_channel"})
    ok(c, {"type": "control", "action": "next_channel"})
    s = c.state(lambda s: (s["now_playing"] or {}).get("text") in ("Build.", "Build, reading again."))
    assert s["now_playing"]["channel"] == "s1"


def test_the_runtime_key_and_previews(rt):
    c = rt.tcp(extensions=["system"])
    info = get(c, "runtime")
    assert info["pid"] == rt.proc.pid
    assert info["config"].endswith("config.json")
    r = ok(c, {"type": "preview", "voice": "Fake tone"})
    assert r["voice"] == "tone"
    r = c.request({"type": "preview", "voice": "nobody"})
    assert r["error"]["code"] == "E_NOT_FOUND"
    status, r = rt.post("preview", {})
    assert status == 200 and r["engine"] == "fake"
