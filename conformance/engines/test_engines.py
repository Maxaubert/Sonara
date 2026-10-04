"""External engines (protocol 1.2, ``docs/protocol-v1.md`` "External
engines"): capability ``engines``, ``engine_add``/``engine_list``/
``engine_remove``/``engine_key``/``engine_test``, speech through a profile,
the fallback with its reason, persistence, and keys that never land in a
home file. A fake OpenAI-compatible server stands in for the provider."""
from __future__ import annotations

import json
import socket

from harness import wait_until

SECRET = "sk-conformance-secret-0123456789"


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


def add(c, profile, secret=SECRET):
    msg = {"type": "engine_add", "engine": profile}
    if secret is not None:
        msg["secret"] = secret
    return ok(c, msg)


def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def test_capability_engines_listed(rt, client):
    assert "engines" in rt.info["capabilities"]
    assert rt.info["protocol"] == {"major": 1, "minor": 5}
    r = client.hello(rt.token, require=["engines"])
    assert r["ok"] is True
    assert "engines" in r["capabilities"]
    lst = ok(client, {"type": "engine_list"})
    assert lst["engines"] == []
    assert lst["builtin"] == ["fake"]
    assert lst["kinds"] == ["openai-compatible", "elevenlabs", "azure", "google", "gemini",
                           "cartesia", "deepgram", "command"]
    assert "openai" in lst["presets"] and "generic" in lst["presets"]


def test_add_list_remove_round_trip(client, profile, provider):
    view = add(client, profile)["engine"]
    assert view["id"] == "local"
    assert view["key_present"] is True
    assert view["sends_text_to"] == "127.0.0.1"
    assert view["local"] is True
    assert view["license_class"] == "external"
    assert view["current"] is False
    assert view["status"]["status"] == "ready"
    assert "secret" not in json.dumps(view)
    r = client.request({"type": "engine_add", "engine": profile})
    assert r["error"]["code"] == "E_BAD_REQUEST"
    lst = ok(client, {"type": "engine_list"})
    assert [e["id"] for e in lst["engines"]] == ["local"]
    voices = ok(client, {"type": "voices", "engine": "local", "refresh": True})["voices"]
    assert [v["id"] for v in voices] == ["af_heart", "am_echo"]
    assert all(v["license_class"] == "external" for v in voices)
    r = ok(client, {"type": "engine_remove", "engine": "local"})
    assert r["removed"] == "local" and r["engine"] == "fake"
    assert ok(client, {"type": "engine_list"})["engines"] == []
    r = client.request({"type": "engine_remove", "engine": "local"})
    assert r["error"]["code"] == "E_NOT_FOUND"


def test_speak_with_profile_hits_the_server_with_bearer(client, profile, provider):
    add(client, profile)
    assert ok(client, {"type": "set", "key": "engine", "value": "local"})["value"] == "local"
    ok(client, {"type": "subscribe", "events": ["items"]})
    item = ok(client, {"type": "speak", "text": "Hello from the fake provider."})["item_id"]
    client.item(item, "finished")
    sent = provider.speech()
    assert sent, "no request reached the provider"
    assert sent[0]["headers"]["authorization"] == f"Bearer {SECRET}"
    body = json.loads(sent[0]["body"])
    assert body["input"] == "Hello from the fake provider."
    # No model named: none sent, the server picks its own (#235).
    assert "model" not in body and body["response_format"] == "wav"
    assert body["voice"] == "af_heart"
    assert body["stream"] is False


def test_server_down_item_finishes_by_fallback_and_status_has_reason(client, profile):
    profile = dict(profile, url=f"http://127.0.0.1:{free_port()}/v1")
    add(client, profile)
    ok(client, {"type": "set", "key": "engine", "value": "local"})
    ok(client, {"type": "subscribe", "events": ["items", "state"]})
    for text in ("First sentence.", "Second sentence."):
        item = ok(client, {"type": "speak", "text": text})["item_id"]
        # The fallback (the fake engine) speaks it: finished, not failed.
        client.item(item, "finished")
    st = client.state(lambda s: s["engine_status"].get("reason") == "network")
    assert st["engine_status"]["engine"] == "local"
    assert st["engine_status"]["status"] == "waiting"
    assert st["engine_status"]["fallback"] == "fake"


def test_profile_and_engine_survive_restart(start, rt, profile, provider):
    c = rt.tcp()
    add(c, profile)
    ok(c, {"type": "set", "key": "engine", "value": "local"})
    c.close()
    rt.close()
    rt2 = start(home=rt.home)
    c2 = rt2.tcp()
    assert ok(c2, {"type": "get", "key": "engine"})["value"] == "local"
    view = ok(c2, {"type": "engine_list"})["engines"][0]
    assert view["id"] == "local" and view["current"] is True and view["key_present"] is True
    c2.close()


def test_secret_not_in_any_home_file(rt, client, profile, provider):
    add(client, profile)
    ok(client, {"type": "engine_key", "engine": "local", "secret": SECRET})
    ok(client, {"type": "engine_test", "engine": "local", "play": False})
    ok(client, {"type": "set", "key": "engine", "value": "local"})
    for p in rt.home.rglob("*"):
        # fake-keys.json is the testing aid that stands in for Credential Manager.
        if p.is_file() and p.name != "fake-keys.json":
            assert SECRET not in p.read_text(encoding="utf-8", errors="replace"), p
    stored = json.loads((rt.home / "engines.json").read_text(encoding="utf-8"))
    assert stored["engines"][0]["key_ref"] == "credman"
    assert SECRET not in rt.stderr()


def test_no_external_engines_flag_refuses(start):
    rt = start("--no-external-engines")
    assert "engines" not in rt.info["capabilities"]
    c = rt.tcp()
    r = c.request({"type": "engine_list"})
    assert r["error"]["code"] == "E_UNSUPPORTED"
    assert r["error"]["message"] == "this runtime does not allow external engines"
    status, body = rt.post("engine_add", {"engine": {"id": "x", "kind": "openai-compatible"}})
    assert status == 400 and body["error"]["code"] == "E_UNSUPPORTED"
    c.close()


def test_engine_test_reports_auth_reason(rt, client, profile, provider):
    add(client, profile)
    r = ok(client, {"type": "engine_test", "engine": "local", "play": False})
    assert r["engine"] == "local" and r["sample_rate"] == 24000 and r["duration_ms"] == 200
    provider.fail(401, {"message": "Incorrect API key provided", "code": "invalid_api_key"})
    r = client.request({"type": "engine_test", "engine": "local"})
    assert r["error"]["code"] == "E_ENGINE"
    assert r["error"]["reason"] == "auth"
    assert "Incorrect API key provided" in r["error"]["message"]
    # Over HTTP too.
    status, body = rt.post("engine_test", {"engine": "local", "play": False})
    assert status == 500 and body["error"]["reason"] == "auth"
    assert wait_until(lambda: True)


KEY_HEADERS = ("authorization", "xi-api-key", "x-goog-api-key", "ocp-apim-subscription-key")


def saw_a_key(p) -> bool:
    with p.lock:
        reqs = list(p.requests)
    return any(
        k in KEY_HEADERS or SECRET in v
        for r in reqs
        for k, v in r["headers"].items()
    )


def try_to_use(rt, c, engine_id):
    """Every way a profile sends a request: test, voices, speech."""
    c.request({"type": "engine_test", "engine": engine_id, "play": False})
    c.request({"type": "voices", "engine": engine_id, "refresh": True})
    ok(c, {"type": "set", "key": "engine", "value": engine_id})
    ok(c, {"type": "subscribe", "events": ["items"]})
    item = ok(c, {"type": "speak", "text": "Where does this go?"})["item_id"]
    c.item(item, "finished")
    rt.post("engine_test", {"engine": engine_id, "play": False})


def test_retargeted_profile_never_sends_the_old_key(rt, client, profile, provider):
    """Spec 6.4: a key is bound to the origin it was entered for. A replace
    to another url (over TCP or HTTP) without a new secret deletes it; the
    new host never sees it."""
    from fake_openai import FakeOpenAI

    add(client, profile)
    for via in ("tcp", "http"):
        other = FakeOpenAI()
        try:
            moved = dict(profile, url=other.url)
            body = {"engine": moved, "replace": True}
            if via == "tcp":
                view = ok(client, dict(body, type="engine_add"))["engine"]
            else:
                status, reply = rt.post("engine_add", body)
                assert status == 200, reply
                view = reply["engine"]
            assert view["key_present"] is False, via
            try_to_use(rt, client, "local")
            assert not saw_a_key(other), via
            r = client.request({"type": "engine_test", "engine": "local", "play": False})
            assert r["error"]["reason"] == "no_key", r
            # Back on the first host, with the key entered again.
            ok(client, {"type": "engine_add", "engine": profile, "replace": True, "secret": SECRET})
        finally:
            other.stop()
    keys = json.loads((rt.home / "fake-keys.json").read_text(encoding="utf-8"))
    assert keys["local"]["origin"] == provider.url[: -len("/v1")]


def test_key_kept_when_only_voice_or_model_change(client, profile, provider):
    add(client, profile)
    view = ok(client, {"type": "engine_add", "replace": True,
                       "engine": dict(profile, voice="am_echo", model="kokoro-2")})["engine"]
    assert view["key_present"] is True
    ok(client, {"type": "engine_test", "engine": "local", "play": False})
    assert provider.speech()[-1]["headers"]["authorization"] == f"Bearer {SECRET}"


def test_new_secret_with_the_new_url_is_used(client, profile, provider):
    from fake_openai import FakeOpenAI

    add(client, profile)
    other = FakeOpenAI()
    try:
        new = "sk-conformance-new-host-0123456789"
        view = ok(client, {"type": "engine_add", "replace": True, "secret": new,
                           "engine": dict(profile, url=other.url)})["engine"]
        assert view["key_present"] is True
        ok(client, {"type": "engine_test", "engine": "local", "play": False})
        assert other.speech()[-1]["headers"]["authorization"] == f"Bearer {new}"
        assert not saw_a_key(provider)
    finally:
        other.stop()


def test_engine_models_and_the_voice_picked_in_sonara(client, profile, provider):
    # #235: the server's models come live; the test of the current engine
    # speaks with the voice picked in Sonara, not the profile's.
    add(client, profile)
    m = ok(client, {"type": "engine_models", "engine": "local", "refresh": True})
    # A server that marks no task: all it lists (OpenAI's own: the tts ones).
    assert [x["id"] for x in m["models"]] == ["tts-a", "tts-b", "chat-c"]
    assert m["list"] is True and m["required"] is False
    r = ok(client, {"type": "engine_test", "engine": "local", "play": False})
    assert r["voice"] == "af_heart"
    ok(client, {"type": "set", "key": "engine", "value": "local"})
    ok(client, {"type": "set", "key": "voice", "value": "am_echo"})
    r = ok(client, {"type": "engine_test", "engine": "local", "play": False})
    assert r["voice"] == "am_echo"
    assert json.loads(provider.speech()[-1]["body"])["voice"] == "am_echo"
