"""Cloud kinds of external engines (#225, ``docs/protocol-v1.md`` "External
engines"): ``elevenlabs``, ``azure`` and ``google``, each against a local
fake of its provider (``fakes.py``). A sentence reaches the provider with
the key in the provider's own header, and a refused key makes the runtime
read with its built-in engine and report ``auth``."""
from __future__ import annotations

import json

import pytest
from fakes import FakeCloud

SECRET = "cloud-conformance-secret-0123456789"
KINDS = ["elevenlabs", "azure", "google"]


def ok(c, msg):
    r = c.request(msg)
    assert r["ok"] is True, (msg, r)
    return r


@pytest.fixture(params=KINDS)
def cloud(request):
    fake = FakeCloud(request.param, SECRET)
    yield fake
    fake.stop()


def test_cloud_kinds_are_listed(client):
    assert ok(client, {"type": "engine_list"})["kinds"] == ["openai-compatible"] + KINDS


def test_speak_reaches_the_provider_with_its_key_header(client, cloud):
    view = ok(client, {"type": "engine_add", "engine": cloud.profile(), "secret": SECRET})["engine"]
    assert view["key_ref"] == "credman" and view["key_present"] is True
    assert view["sends_text_to"] == "127.0.0.1"
    assert view["license_class"] == "external"
    eid = view["id"]
    voices = ok(client, {"type": "voices", "engine": eid, "refresh": True})["voices"]
    assert voices and all(v["license_class"] == "external" for v in voices)
    ok(client, {"type": "set", "key": "engine", "value": eid})
    ok(client, {"type": "subscribe", "events": ["items"]})
    item = ok(client, {"type": "speak", "text": "Hello from the cloud."})["item_id"]
    client.item(item, "finished")
    sent = cloud.speech()
    assert sent, "no request reached the provider"
    req = sent[0]
    assert req["headers"][cloud.shape.key_header] == SECRET
    assert "authorization" not in req["headers"]
    assert SECRET not in req["path"]
    if cloud.shape.kind == "elevenlabs":
        assert req["path"] == "/v1/text-to-speech/voice-a?output_format=pcm_24000"
        body = json.loads(req["body"])
        assert body["text"] == "Hello from the cloud."
        assert body["model_id"] == "eleven_flash_v2_5"
    elif cloud.shape.kind == "azure":
        assert req["headers"]["content-type"] == "application/ssml+xml"
        assert req["headers"]["x-microsoft-outputformat"] == "raw-24khz-16bit-mono-pcm"
        ssml = req["body"].decode()
        assert "<voice name='en-US-AvaMultilingualNeural'>" in ssml
        assert "Hello from the cloud." in ssml
    else:
        body = json.loads(req["body"])
        assert body["input"] == {"text": "Hello from the cloud."}
        assert body["audioConfig"]["audioEncoding"] == "PCM"
        assert body["voice"] == {"languageCode": "en-US", "name": "en-US-Chirp3-HD-Kore"}
    r = ok(client, {"type": "engine_test", "engine": eid, "play": False})
    assert r["sample_rate"] == 24000 and r["duration_ms"] == 200


def test_refused_key_falls_back_and_reports_auth(client, cloud):
    eid = ok(client, {"type": "engine_add", "engine": cloud.profile(), "secret": SECRET})["engine"]["id"]
    cloud.refuse_key()
    r = client.request({"type": "engine_test", "engine": eid, "play": False})
    assert r["error"]["code"] == "E_ENGINE"
    assert r["error"]["reason"] == "auth"
    assert SECRET not in json.dumps(r)
    ok(client, {"type": "set", "key": "engine", "value": eid})
    ok(client, {"type": "subscribe", "events": ["items", "state"]})
    item = ok(client, {"type": "speak", "text": "Read by the fallback."})["item_id"]
    # The fallback (the fake engine) speaks it: finished, not failed.
    client.item(item, "finished")
    st = client.state(lambda s: s["engine_status"].get("reason") == "auth")
    assert st["engine_status"]["engine"] == eid
    assert st["engine_status"]["status"] == "unavailable"
    assert st["engine_status"]["fallback"] == "fake"

