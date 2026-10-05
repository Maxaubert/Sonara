"""Cloud kinds of external engines (#225, #226, ``docs/protocol-v1-engines.md``
"Kinds"): ``elevenlabs``, ``azure``, ``google``, ``gemini``, ``cartesia`` and
``deepgram``, each against a local fake of its provider (``fakes.py``). A sentence reaches the provider with
the key in the provider's own header, and a refused key makes the runtime
read with its built-in engine and report ``auth``."""
from __future__ import annotations

import json

import pytest
from fakes import FakeCloud

SECRET = "cloud-conformance-secret-0123456789"
KINDS = ["elevenlabs", "azure", "google", "gemini", "cartesia", "deepgram"]


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
    assert ok(client, {"type": "engine_list"})["kinds"] == ["openai-compatible"] + KINDS + ["command"]


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
    assert req["headers"][cloud.shape.key_header] == cloud.shape.key_prefix + SECRET
    if cloud.shape.key_header != "authorization":
        assert "authorization" not in req["headers"]
    assert SECRET not in req["path"]
    if cloud.shape.kind == "elevenlabs":
        # A whole message (send mode `message`, the cloud default, #235):
        # the stream endpoint, its PCM played as it comes.
        assert req["path"] == "/v1/text-to-speech/voice-a/stream?output_format=pcm_24000"
        body = json.loads(req["body"])
        assert body["text"] == "Hello from the cloud."
        # No model named: none sent, ElevenLabs uses its own (#235).
        assert "model_id" not in body
    elif cloud.shape.kind == "azure":
        assert req["headers"]["content-type"] == "application/ssml+xml"
        assert req["headers"]["x-microsoft-outputformat"] == "raw-24khz-16bit-mono-pcm"
        ssml = req["body"].decode()
        assert "<voice name='en-US-VoiceZNeural'>" in ssml
        assert "Hello from the cloud." in ssml
    elif cloud.shape.kind == "cartesia":
        assert req["path"] == "/tts/bytes"
        assert req["headers"]["cartesia-version"] == "2026-08-14"
        body = json.loads(req["body"])
        assert body["transcript"] == "Hello from the cloud."
        assert body["model_id"] == "model-c"
        assert body["voice"] == {"id": "voice-c"}
        assert body["output_format"] == {"container": "raw", "encoding": "pcm_s16le",
                                         "sample_rate": 24000}
    elif cloud.shape.kind == "gemini":
        # The key only in x-goog-api-key, never in the URL; the model the
        # user picked, streamed; the voices from Google's list (#235).
        assert req["path"] == "/v1beta/models/tts-g:streamGenerateContent?alt=sse"
        assert [v["id"] for v in voices] == ["voice-g"]
        assert any(r["path"].startswith("/v1beta/voices?") for r in cloud.requests)
        body = json.loads(req["body"])
        # Gemini has no speed; at the product rate (250 wpm) no style is
        # sent, so the model reads at its own pace.
        assert body["contents"] == [{"role": "user", "parts": [
            {"text": "Hello from the cloud."}]}]
        gen = body["generationConfig"]
        assert gen["responseModalities"] == ["AUDIO"]
        assert gen["speechConfig"] == {"voiceConfig": {"prebuiltVoiceConfig": {"voiceName": "voice-g"}}}
        assert gen["responseFormat"] == {"audio": {"mimeType": "AUDIO_L16", "sampleRate": 24000}}
    elif cloud.shape.kind == "deepgram":
        assert req["path"].startswith("/v1/speak?model=voice-d-en&encoding=linear16"
                                      "&container=none&sample_rate=24000")
        assert json.loads(req["body"]) == {"text": "Hello from the cloud."}
    else:
        body = json.loads(req["body"])
        assert body["input"] == {"text": "Hello from the cloud."}
        assert body["audioConfig"]["audioEncoding"] == "PCM"
        assert body["voice"] == {"languageCode": "en-US", "name": "en-US-Voice-G"}
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


def test_gemini_models_and_voices_come_live_and_nothing_is_preset(client):
    # #235: no model or voice in Sonara. A Gemini profile without them is
    # kept and says "choose"; its models and voices come from Google's
    # lists (here the fake's), for a saved profile and for the form's draft.
    cloud = FakeCloud("gemini", SECRET)
    try:
        bare = {"id": "ge", "kind": "gemini", "url": cloud.url, "options": {"timeout_ms": 5000}}
        view = ok(client, {"type": "engine_add", "engine": bare, "secret": SECRET})["engine"]
        assert "model" not in view and "voice" not in view
        assert view["missing"] == ["model", "voice"]
        assert view["model_required"] is True and view["model_list"] is True
        assert view["status"]["reason"] == "bad_config"
        assert view["status"]["message"].startswith("Choose a model")
        r = client.request({"type": "engine_test", "engine": "ge", "play": False})
        assert r["error"]["reason"] == "bad_config"
        assert cloud.speech() == [], "nothing sent without a model"
        m = ok(client, {"type": "engine_models", "engine": "ge", "refresh": True})
        assert m["models"] == [{"id": "tts-g", "name": "TTS G"}]
        assert m["list"] is True and m["required"] is True
        draft = dict(bare)
        del draft["id"]
        m = ok(client, {"type": "engine_models", "profile": draft, "secret": SECRET})
        assert [x["id"] for x in m["models"]] == ["tts-g"]
        v = ok(client, {"type": "voices", "profile": draft, "secret": SECRET})
        assert [x["id"] for x in v["voices"]] == ["voice-g"]
        # Picked: it reads, streamed.
        picked = dict(bare, model="tts-g", voice="voice-g")
        view = ok(client, {"type": "engine_add", "engine": picked, "replace": True})["engine"]
        assert view["missing"] == []
        r = ok(client, {"type": "engine_test", "engine": "ge", "play": False})
        assert r["voice"] == "voice-g" and r["duration_ms"] == 200
        assert cloud.speech()[-1]["path"] == "/v1beta/models/tts-g:streamGenerateContent?alt=sse"
    finally:
        cloud.stop()
