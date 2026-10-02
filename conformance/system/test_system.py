"""Extension ``system`` (spec 4.4): other apps' audio while speech plays
(``audio_mode``, ``duck_level``), hotkeys, the settings page, and the rule
that other apps are never left ducked or paused: restored when speech
pauses or ends, when the client that needed the extension leaves
(restore_on_client_drop) and, after a crash, by the next runtime's startup
sweep (restore_after_runtime_kill).

The runtime runs with ``--system fake``: the "apps" are entries in
``<home>/fake-system.json`` that the test writes and reads, never this PC's
audio, media or hotkeys. Ported rules of the Python ``test_ducking.py``,
``test_pausing.py`` and ``test_keymap.py`` at the protocol level."""
from __future__ import annotations

import http.client
import json
import time
import urllib.parse

from harness import TIMEOUT, long_text, wait_until

LONG = long_text(3)
DEFAULT_COMBOS = {"restart": "Ctrl+Alt+Up", "flush": "Ctrl+Alt+Down", "mute": "Ctrl+Alt+M", "next_channel": "Ctrl+Alt+P"}


def write_world(path, world):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(world), encoding="utf-8")


def read_world(path) -> dict:
    """The fake world; retried while the runtime is replacing the file."""
    end = time.monotonic() + TIMEOUT
    while True:
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            if time.monotonic() > end:
                raise
            time.sleep(0.02)


def volume(rt, pid):
    return next(a["volume"] for a in read_world(rt.fake_system)["audio"] if a["pid"] == pid)


def media(rt, app):
    return next(m for m in read_world(rt.fake_system)["media"] if m["app"] == app)


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def system_client(rt, **fields):
    return rt.tcp(extensions=["system"], **fields)


def set_key(c, key, value):
    return ok(c, {"type": "set", "key": key, "value": value})["value"]


def apps_world(**extra):
    world = {"audio": [{"pid": 100, "name": "vlc.exe", "volume": 0.8}, {"pid": 200, "name": "audiodg.exe", "volume": 1.0}], "media": [{"app": "spotify", "playing": True}]}
    world.update(extra)
    return world


def start_with_apps(start, tmp_path, **extra):
    home = tmp_path / "home"
    write_world(home / "fake-system.json", apps_world(**extra))
    return start(home=home)


def speak_long(c):
    ok(c, {"type": "subscribe", "events": ["state"]})
    ok(c, {"type": "speak", "text": LONG})
    c.state(lambda s: s["now_playing"] is not None)


def near(a, b):
    return abs(a - b) < 1e-3


# --- the extension surface ----------------------------------------------------


def test_system_is_offered_and_enabled_by_hello(rt):
    assert "system" in rt.read_runtime()["extensions"]
    c = rt.tcp()
    r = c.request({"type": "get", "key": "audio_mode"})
    assert r["error"]["code"] == "E_UNSUPPORTED"
    r = c.hello(rt.token, extensions=["system"])
    assert r["ok"] and "system" in r["extensions"]
    assert ok(c, {"type": "get", "key": "audio_mode"})["value"] == "off"
    assert ok(c, {"type": "get", "key": "duck_level"})["value"] == 30


def test_audio_mode_and_duck_level_validation(rt):
    c = system_client(rt)
    for mode in ("duck", "pause", "off"):
        assert set_key(c, "audio_mode", mode) == mode
    assert set_key(c, "duck_level", 0) == 0
    assert set_key(c, "duck_level", 100) == 100
    for key, value in (("audio_mode", "loud"), ("audio_mode", 1), ("duck_level", 101), ("duck_level", -1), ("duck_level", "x"), ("settings_url", "x")):
        r = c.request({"type": "set", "key": key, "value": value})
        assert r["error"]["code"] == "E_BAD_REQUEST", (key, value, r)


# --- ducking and pausing ------------------------------------------------------


def test_duck_while_speaking_and_restore_when_it_stops(start, tmp_path):
    rt = start_with_apps(start, tmp_path)
    world = read_world(rt.fake_system)
    world["audio"].append({"pid": rt.proc.pid, "name": "sonarad.exe", "volume": 1.0})
    write_world(rt.fake_system, world)
    c = system_client(rt)
    set_key(c, "audio_mode", "duck")
    set_key(c, "duck_level", 20)
    speak_long(c)
    assert wait_until(lambda: near(volume(rt, 100), 0.2))
    assert volume(rt, 200) == 1.0, "the audio engine is never ducked"
    assert volume(rt, rt.proc.pid) == 1.0, "nor the runtime's own audio"
    ok(c, {"type": "control", "action": "pause"})
    assert wait_until(lambda: near(volume(rt, 100), 0.8)), "a pause restores at once"
    ok(c, {"type": "control", "action": "play"})
    assert wait_until(lambda: near(volume(rt, 100), 0.2))
    ok(c, {"type": "control", "action": "stop"})
    assert wait_until(lambda: near(volume(rt, 100), 0.8))
    assert wait_until(lambda: not (rt.home / "state" / "duck_state.json").exists())


def test_pause_mode_pauses_media_and_resumes_it(start, tmp_path):
    rt = start_with_apps(start, tmp_path)
    c = system_client(rt)
    set_key(c, "audio_mode", "pause")
    speak_long(c)
    assert wait_until(lambda: media(rt, "spotify")["playing"] is False)
    assert volume(rt, 100) == 0.8, "pause mode does not duck"
    ok(c, {"type": "control", "action": "stop"})
    assert wait_until(lambda: media(rt, "spotify")["playing"] is True)


def test_a_mode_switch_while_speaking_restores_the_other_backend(start, tmp_path):
    rt = start_with_apps(start, tmp_path)
    c = system_client(rt)
    set_key(c, "audio_mode", "duck")
    speak_long(c)
    assert wait_until(lambda: near(volume(rt, 100), 0.3))
    set_key(c, "audio_mode", "pause")
    assert wait_until(lambda: near(volume(rt, 100), 0.8) and media(rt, "spotify")["playing"] is False)
    set_key(c, "audio_mode", "off")
    assert wait_until(lambda: media(rt, "spotify")["playing"] is True)


def test_restore_on_client_drop(start, tmp_path):
    rt = start_with_apps(start, tmp_path)
    c = system_client(rt)
    set_key(c, "audio_mode", "duck")
    speak_long(c)
    assert wait_until(lambda: near(volume(rt, 100), 0.3))
    observer = rt.tcp()
    c.close()
    assert wait_until(lambda: near(volume(rt, 100), 0.8)), "restored when the client left"
    st = ok(observer, {"type": "subscribe", "events": ["state"]})
    assert st["ok"]
    assert observer.state()["now_playing"] is not None, "the reader keeps reading"


def test_another_client_that_needs_it_keeps_the_duck(start, tmp_path):
    rt = start_with_apps(start, tmp_path)
    a = system_client(rt)
    b = system_client(rt)
    set_key(a, "audio_mode", "duck")
    speak_long(a)
    assert wait_until(lambda: near(volume(rt, 100), 0.3))
    b.close()
    time.sleep(0.3)
    assert near(volume(rt, 100), 0.3)
    a.close()
    assert wait_until(lambda: near(volume(rt, 100), 0.8))


def test_restore_after_runtime_kill(start, tmp_path):
    home = tmp_path / "home"
    write_world(home / "fake-system.json", apps_world())
    rt = start(home=home)
    c = system_client(rt)
    set_key(c, "audio_mode", "duck")
    speak_long(c)
    assert wait_until(lambda: near(volume(rt, 100), 0.3))
    rt.proc.kill()
    rt.wait_exit()
    assert near(volume(rt, 100), 0.3), "a killed runtime cannot restore"
    record = json.loads((home / "state" / "duck_state.json").read_text(encoding="utf-8"))
    assert record["sessions"][0]["pid"] == 100 and near(record["sessions"][0]["original"], 0.8)
    # The next runtime on this home sweeps at startup, before any client.
    rt2 = start(home=home)
    assert near(volume(rt2, 100), 0.8)
    assert not (home / "state" / "duck_state.json").exists()


def test_paused_media_is_resumed_after_runtime_kill(start, tmp_path):
    home = tmp_path / "home"
    write_world(home / "fake-system.json", apps_world())
    rt = start(home=home)
    c = system_client(rt)
    set_key(c, "audio_mode", "pause")
    speak_long(c)
    assert wait_until(lambda: media(rt, "spotify")["playing"] is False)
    rt.proc.kill()
    rt.wait_exit()
    rt2 = start(home=home)
    assert media(rt2, "spotify")["playing"] is True
    assert not (home / "state" / "pause_state.json").exists()


def test_the_sweep_keeps_what_it_could_not_restore(start, tmp_path):
    home = tmp_path / "home"
    write_world(home / "fake-system.json", {"audio": [{"pid": 100, "name": "vlc.exe", "volume": 0.3, "broken": True}]})
    (home / "state").mkdir(parents=True)
    (home / "state" / "duck_state.json").write_text(json.dumps({"sessions": [{"pid": 100, "name": "vlc.exe", "original": 0.8}]}), encoding="utf-8")
    start(home=home)
    record = json.loads((home / "state" / "duck_state.json").read_text(encoding="utf-8"))
    assert record["sessions"][0]["name"] == "vlc.exe"


# --- hotkeys ------------------------------------------------------------------


def hotkeys(c):
    return ok(c, {"type": "get", "key": "hotkeys"})["value"]


def binding(value, action):
    return next(b for b in value["bindings"] if b["action"] == action)


def test_hotkeys_are_registered_only_while_a_client_needs_them(rt):
    c = system_client(rt)
    v = hotkeys(c)
    assert v["active"] is True
    for action, combo in DEFAULT_COMBOS.items():
        b = binding(v, action)
        assert b["combo"] == combo and b["registered"] is True, b
    assert binding(v, "pause")["key"] is None, "pause, faster and slower ship unbound"
    assert len(read_world(rt.fake_system)["hotkeys"]) == 4
    c.close()
    assert wait_until(lambda: read_world(rt.fake_system)["hotkeys"] == [])
    status, r = rt.post("hello", {"extensions": ["system"]})
    assert status == 200
    status, r = rt.post("get", {"key": "hotkeys"})
    assert r["value"]["active"] is False, "an HTTP request does not hold the hotkeys"


def test_keep_alive_holds_the_hotkeys_after_the_client_left(rt):
    c = system_client(rt, keep_alive=True)
    c.close()
    time.sleep(0.3)
    assert len(read_world(rt.fake_system)["hotkeys"]) == 4


def test_bind_unbind_reset_and_the_keymap_file(rt):
    c = system_client(rt)
    v = set_key(c, "hotkeys", {"action": "mute", "key": "k", "mods": ["win", "alt"]})
    assert binding(v, "mute")["combo"] == "Win+Alt+K" and binding(v, "mute")["registered"] is True
    on_disk = json.loads((rt.home / "keymap.json").read_text(encoding="utf-8"))
    assert on_disk["mute"] == {"key": "k", "mods": ["win", "alt"]}
    registered = read_world(rt.fake_system)["hotkeys"]
    assert any(h["vk"] == ord("K") and h["mods"] & 0xF == 0x9 for h in registered)
    v = set_key(c, "hotkeys", {"action": "restart", "key": None})
    assert binding(v, "restart")["key"] is None
    v = set_key(c, "hotkeys", "reset")
    for action, combo in DEFAULT_COMBOS.items():
        assert binding(v, action)["combo"] == combo


def test_bad_hotkeys_are_refused_before_anything_is_written(rt):
    c = system_client(rt)
    for value in (
        {"action": "mute", "key": "m", "mods": []},
        {"action": "mute", "key": "m", "mods": ["shift"]},
        {"action": "mute", "key": "escape", "mods": ["ctrl", "alt"]},
        {"action": "mute", "key": "m", "mods": ["hyper"]},
        {"action": "warp", "key": "m", "mods": ["ctrl"]},
        "everything",
    ):
        r = c.request({"type": "set", "key": "hotkeys", "value": value})
        assert r["error"]["code"] == "E_BAD_REQUEST", (value, r)
    assert not (rt.home / "keymap.json").exists()


def test_a_chord_another_program_owns_is_reported(start, tmp_path):
    rt = start_with_apps(start, tmp_path, taken=[{"mods": 3, "vk": 0x50}])
    v = hotkeys(system_client(rt))
    b = binding(v, "next_channel")
    assert b["registered"] is False and b["error"] == "already_owned"
    assert binding(v, "restart")["registered"] is True


def test_altgr_clash_is_a_warning(start, tmp_path):
    # E16: German AltGr+M types the micro sign, so Ctrl+Alt+M eats it.
    rt = start_with_apps(start, tmp_path, altgr=[{"vk": 0x4D, "shift": False, "char": "µ"}])
    v = hotkeys(system_client(rt))
    assert binding(v, "mute")["altgr"] == "µ"
    assert binding(v, "restart")["altgr"] is None


# --- the settings page --------------------------------------------------------


def get_page(rt, path, host=None):
    conn = http.client.HTTPConnection("127.0.0.1", rt.info["http_port"], timeout=TIMEOUT)
    conn.putrequest("GET", path, skip_host=True)
    conn.putheader("Host", host or f"127.0.0.1:{rt.info['http_port']}")
    conn.endheaders()
    r = conn.getresponse()
    body = r.read().decode("utf-8", "replace")
    headers = {k.lower(): v for k, v in r.getheaders()}
    conn.close()
    return r.status, headers, body


def test_settings_page_is_served_with_the_token_only_to_its_url(rt):
    status, _, _ = get_page(rt, f"/settings?token={rt.token}")
    assert status == 404, "no page before a client enabled the extension"
    c = system_client(rt)
    url = ok(c, {"type": "get", "key": "settings_url"})["value"]
    parsed = urllib.parse.urlsplit(url)
    assert parsed.hostname == "127.0.0.1" and parsed.port == rt.info["http_port"]
    status, headers, body = get_page(rt, parsed.path + "?" + parsed.query)
    assert status == 200
    assert headers["content-type"].startswith("text/html")
    assert "frame-ancestors 'none'" in headers["content-security-policy"]
    assert headers["referrer-policy"] == "no-referrer"
    assert headers["cache-control"] == "no-store"
    assert "access-control-allow-origin" not in headers
    assert f'"{rt.token}"' in body and "__SONARA_TOKEN__" not in body
    assert "/v1/" in body and "/api/" not in body
    assert get_page(rt, "/settings")[0] == 401
    assert get_page(rt, "/settings?token=wrong")[0] == 401
    assert get_page(rt, f"/settings?token={rt.token}", host=f"evil.example:{rt.info['http_port']}")[0] == 403


def test_the_api_sends_no_cors_headers(rt):
    conn = http.client.HTTPConnection("127.0.0.1", rt.info["http_port"], timeout=TIMEOUT)
    conn.request("POST", "/v1/get", body=json.dumps({"key": "volume"}), headers={"Authorization": f"Bearer {rt.token}", "Origin": "https://evil.example"})
    r = conn.getresponse()
    r.read()
    assert r.status == 200
    assert r.getheader("Access-Control-Allow-Origin") is None
    conn.close()


# -- spoken control cues (#197) --------------------------------------------


def cue(c, timeout=5.0):
    return c.next_event(lambda e: e.get("event") == "cue", timeout)["text"]


def test_setting_changes_speak_their_cues(rt):
    c = rt.tcp(extensions=["system", "agent"])
    ok(c, {"type": "subscribe", "events": ["cues"]})
    set_key(c, "audio_mode", "duck")
    assert cue(c) == "Audio ducking."
    set_key(c, "duck_level", 45)
    assert cue(c) == "Duck level 45 percent."
    set_key(c, "mute_level", 2)
    assert cue(c) == "Super muted."
    set_key(c, "mute_level", 0)
    assert cue(c) == "Unmuted."


def test_resending_a_setting_unchanged_speaks_no_cue(rt):
    c = rt.tcp(extensions=["system", "agent"])
    ok(c, {"type": "subscribe", "events": ["cues"]})
    set_key(c, "audio_mode", "pause")
    assert cue(c) == "Media pause."
    set_key(c, "duck_level", 45)
    assert cue(c) == "Duck level 45 percent."
    set_key(c, "audio_mode", "pause")
    set_key(c, "duck_level", 45)
    set_key(c, "mute_level", 1)
    assert cue(c) == "Muted.", "an unchanged audio_mode or duck_level is silent"


def test_cues_need_the_system_extension(rt):
    c = rt.tcp()
    r = c.request({"type": "subscribe", "events": ["cues"]})
    assert r["error"]["code"] == "E_UNSUPPORTED"


def test_a_cue_over_a_paused_reader_leaves_it_paused(rt):
    c = rt.tcp(extensions=["system", "agent"])
    ok(c, {"type": "subscribe", "events": ["state", "cues"]})
    ok(c, {"type": "speak", "text": LONG})
    c.state(lambda s: s["now_playing"] is not None)
    ok(c, {"type": "control", "action": "pause"})
    c.state(lambda s: s["paused"] is True)
    set_key(c, "mute_level", 1)
    assert cue(c) == "Muted."
    set_key(c, "mute_level", 0)
    assert cue(c) == "Unmuted."
