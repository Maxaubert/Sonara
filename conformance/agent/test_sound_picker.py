"""Extension ``agent``: the sound picker (#211). ``get earcons`` lists the
bundled library and what each earcon plays; ``earcon_select``,
``earcon_upload``, ``earcon_delete`` and ``earcon_preview`` change and play
them. Selections persist in ``config.json`` (``earcon_sounds``) and an own
sound is saved as ``<home>/earcons/<kind>.wav``."""
from __future__ import annotations

import base64
import json
import struct

KINDS = ["choice", "permission", "error", "turn_done", "nav", "nav_edge",
         "session_change", "summary_failed"]


def wav(frames=800, rate=8000, channels=1, value=8000) -> bytes:
    """A 16-bit PCM WAV of a square wave (``value`` 0: silent)."""
    data = b"".join(
        struct.pack("<h", value if (i // 8) % 2 == 0 else -value) * channels for i in range(frames))
    fmt = struct.pack("<HHIIHH", 1, channels, rate, rate * 2 * channels, 2 * channels, 16)
    body = b"WAVEfmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(data)) + data
    return b"RIFF" + struct.pack("<I", len(body)) + body


def b64(b: bytes) -> str:
    return base64.b64encode(b).decode("ascii")


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def err(c, msg):
    r = c.request(msg)
    assert r["ok"] is False, (msg, r)
    return r["error"]["code"]


def earcons(c):
    return ok(c, {"type": "get", "key": "earcons"})["value"]


def saved(rt):
    p = rt.home / "config.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else {}


def test_get_earcons_lists_the_library_and_each_kind(rt):
    c = rt.tcp(extensions=["agent"])
    v = earcons(c)
    assert v["kinds"] == KINDS
    assert v["library"], "the library has sounds"
    for s in v["library"]:
        assert s["id"] and s["label"]
    ids = {"library:" + s["id"] for s in v["library"]}
    assert set(v["events"]) == set(KINDS)
    for kind, e in v["events"].items():
        assert e["selection"] is None, kind
        assert e["default"] in ids | {"none"}, kind
        assert e["effective"] == e["default"], kind
        assert e["custom"] is False, kind
    assert v["max_upload_bytes"] == 1024 * 1024
    assert v["max_seconds"] == 10


def test_select_persists_and_survives_a_restart(start, tmp_path):
    home = tmp_path / "home"
    rt = start(home=home)
    c = rt.tcp(extensions=["agent"])
    sound = earcons(c)["library"][-1]["id"]
    r = ok(c, {"type": "earcon_select", "kind": "turn_done", "source": "library:" + sound})
    assert r["earcons"]["events"]["turn_done"]["effective"] == "library:" + sound
    ok(c, {"type": "earcon_select", "kind": "nav", "source": "none"})
    assert saved(rt)["earcon_sounds"] == {"turn_done": "library:" + sound, "nav": "none"}
    c.close()
    rt.close()
    rt = start(home=home)
    c = rt.tcp(extensions=["agent"])
    events = earcons(c)["events"]
    assert events["turn_done"]["selection"] == "library:" + sound
    assert events["turn_done"]["effective"] == "library:" + sound
    assert events["nav"]["effective"] == "none"
    # Default goes back to the bundled default and is the user's choice too.
    r = ok(c, {"type": "earcon_select", "kind": "turn_done", "source": "default"})
    e = r["earcons"]["events"]["turn_done"]
    assert e["effective"] == e["default"] and e["selection"] == "default"


def test_a_silent_event_is_still_reported(rt):
    c = rt.tcp(extensions=["agent"])
    assert c.request({"type": "subscribe", "events": ["earcons"]})["ok"]
    ok(c, {"type": "earcon_select", "kind": "nav_edge", "source": "none"})
    ok(c, {"type": "earcon", "kind": "nav_edge"})
    assert c.next_event(lambda e: e.get("event") == "earcon")["kind"] == "nav_edge"


def test_select_refuses_bad_requests(rt):
    c = rt.tcp(extensions=["agent"])
    assert err(c, {"type": "earcon_select", "kind": "ready", "source": "none"}) == "E_BAD_REQUEST"
    assert err(c, {"type": "earcon_select", "source": "none"}) == "E_BAD_REQUEST"
    assert err(c, {"type": "earcon_select", "kind": "nav"}) == "E_BAD_REQUEST"
    assert err(c, {"type": "earcon_select", "kind": "nav", "source": "loud"}) == "E_BAD_REQUEST"
    assert err(c, {"type": "earcon_select", "kind": "nav", "source": 3}) == "E_BAD_REQUEST"
    assert err(c, {"type": "earcon_select", "kind": "nav", "source": "library:nope"}) == "E_NOT_FOUND"
    assert err(c, {"type": "earcon_select", "kind": "nav", "source": "custom"}) == "E_NOT_FOUND"
    assert "earcon_sounds" not in saved(rt)


def test_upload_saves_a_mono_wav_and_selects_it(rt):
    c = rt.tcp(extensions=["agent"])
    r = ok(c, {"type": "earcon_upload", "kind": "choice", "wav": b64(wav(channels=2, rate=22050, frames=2205))})
    e = r["earcons"]["events"]["choice"]
    assert (e["selection"], e["effective"], e["custom"]) == ("custom", "custom", True)
    assert r["earcons"]["custom"] == ["choice"]
    f = rt.home / "earcons" / "choice.wav"
    b = f.read_bytes()
    assert b[:4] == b"RIFF"
    channels, rate = struct.unpack("<HI", b[22:28])
    assert (channels, rate) == (1, 22050)
    assert saved(rt)["earcon_sounds"] == {"choice": "custom"}
    r = ok(c, {"type": "earcon_preview", "kind": "choice"})
    assert r["played"] is True and r["source"] == "custom"


def test_bad_uploads_are_refused_and_nothing_is_saved(rt):
    c = rt.tcp(extensions=["agent"])
    cases = [
        {"kind": "nav"},
        {"kind": "nav", "wav": 12},
        {"kind": "nav", "wav": "not base64 !!"},
        {"kind": "nav", "wav": b64(b"RIFF....WAVEjunk")},
        {"kind": "nav", "wav": b64(wav(value=0))},
        {"kind": "nav", "wav": b64(wav(frames=8000 * 11))},
        {"kind": "nav", "wav": b64(b"\0" * (1024 * 1024 + 1))},
        {"kind": "ready", "wav": b64(wav())},
        {"wav": b64(wav())},
    ]
    for body in cases:
        assert err(c, {"type": "earcon_upload", **body}) == "E_BAD_REQUEST", body
    assert not (rt.home / "earcons" / "nav.wav").exists()
    assert earcons(c)["custom"] == []


def test_an_oversize_upload_over_http_is_refused(rt):
    status, r = rt.post("hello", {"extensions": ["agent"]})
    assert status == 200
    big = b64(wav(frames=48000 * 9, rate=48000, channels=1))  # ~860 KB: under the cap
    status, r = rt.post("earcon_upload", {"kind": "turn_done", "wav": big})
    assert status == 200, r
    status, r = rt.post("earcon_upload", {"kind": "turn_done", "wav": "A" * (1500 * 1024)})
    assert status == 400 and r["error"]["code"] == "E_BAD_REQUEST"


def test_delete_removes_the_file_and_its_selection(rt):
    c = rt.tcp(extensions=["agent"])
    ok(c, {"type": "earcon_upload", "kind": "session_change", "wav": b64(wav())})
    r = ok(c, {"type": "earcon_delete", "kind": "session_change"})
    assert r["deleted"] is True
    e = r["earcons"]["events"]["session_change"]
    assert e["selection"] is None and e["custom"] is False and e["effective"] == e["default"]
    assert not (rt.home / "earcons" / "session_change.wav").exists()
    assert "earcon_sounds" not in saved(rt)
    assert ok(c, {"type": "earcon_delete", "kind": "session_change"})["deleted"] is False
    assert err(c, {"type": "earcon_delete", "kind": "ready"}) == "E_BAD_REQUEST"
    assert err(c, {"type": "earcon_delete"}) == "E_BAD_REQUEST"


def test_a_file_in_the_folder_is_custom_until_another_sound_is_picked(rt):
    folder = rt.home / "earcons"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "nav.wav").write_bytes(wav())
    c = rt.tcp(extensions=["agent"])
    e = earcons(c)["events"]["nav"]
    assert (e["selection"], e["effective"], e["custom"]) == (None, "custom", True)
    r = ok(c, {"type": "earcon_select", "kind": "nav", "source": "none"})
    assert r["earcons"]["events"]["nav"]["effective"] == "none"
    assert (folder / "nav.wav").exists(), "picking another sound keeps the file"
    r = ok(c, {"type": "earcon_select", "kind": "nav", "source": "custom"})
    assert r["earcons"]["events"]["nav"]["effective"] == "custom"


def test_preview_plays_library_default_and_current_sounds(rt):
    c = rt.tcp(extensions=["agent"])
    v = earcons(c)
    sound = "library:" + v["library"][0]["id"]
    r = ok(c, {"type": "earcon_preview", "source": sound})
    assert r["played"] is True and r["source"] == sound
    r = ok(c, {"type": "earcon_preview", "kind": "turn_done", "source": "default"})
    assert r["played"] is (v["events"]["turn_done"]["default"] != "none")
    r = ok(c, {"type": "earcon_preview", "kind": "error"})
    assert r["source"] == v["events"]["error"]["effective"]
    r = ok(c, {"type": "earcon_preview", "kind": "nav", "source": "none"})
    assert r["played"] is False
    # Mute level 2 silences earcons, not a preview the user asked for.
    ok(c, {"type": "set", "key": "mute_level", "value": 2})
    assert ok(c, {"type": "earcon_preview", "source": sound})["played"] is True
    assert err(c, {"type": "earcon_preview"}) == "E_BAD_REQUEST"
    assert err(c, {"type": "earcon_preview", "kind": "ready"}) == "E_BAD_REQUEST"
    assert err(c, {"type": "earcon_preview", "source": "loud"}) == "E_BAD_REQUEST"
    assert err(c, {"type": "earcon_preview", "source": "library:nope"}) == "E_NOT_FOUND"
    assert err(c, {"type": "earcon_preview", "source": "custom"}) == "E_NOT_FOUND"
    assert err(c, {"type": "earcon_preview", "kind": "nav", "source": "custom"}) == "E_NOT_FOUND"


def test_the_picker_needs_the_agent_extension(rt):
    c = rt.tcp()
    for msg in ({"type": "earcon_select", "kind": "nav", "source": "none"},
                {"type": "earcon_upload", "kind": "nav", "wav": b64(wav())},
                {"type": "earcon_delete", "kind": "nav"},
                {"type": "earcon_preview", "kind": "nav"}):
        assert err(c, msg) == "E_UNSUPPORTED", msg
