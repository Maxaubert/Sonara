//! Contract tests (#275) of kind `openai-compatible` and its presets
//! against each server's own published schema (`tests/contracts/<preset>`,
//! sources in each `SOURCES.md`): the request the adapter really sends (a
//! scripted local server captures it) is validated against the schema
//! fragment, and spec-shaped answers (audio, voice and model lists, error
//! bodies) go through the adapter's parsing and error mapping. No real
//! provider is called. The `generic` preset has no spec of its own.
mod common;

use common::contract::{self, engine, pcm_bytes, reply, speak, Fragment, Mode};
use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::adapter::Adapter;
use sonara_engine::external::openai::OpenAi;
use sonara_engine::{Engine, Reason};

const KEY: &str = "sk-contract-0123456789";

fn profile(server: &ScriptServer, preset: &str, extra: Value) -> Value {
    let mut v = json!({"id": "oa", "kind": "openai-compatible",
        "url": format!("{}/v1", server.base), "send_mode": "sentence",
        "options": {"preset": preset, "timeout_ms": 5000}});
    for (k, x) in extra.as_object().unwrap() {
        if k == "options" {
            for (ok, ox) in x.as_object().unwrap() {
                v["options"][ok] = ox.clone();
            }
        } else {
            v[k] = x.clone();
        }
    }
    v
}

fn adapter(preset: &str, extra: Value) -> OpenAi {
    let server_less = json!({"id": "oa", "kind": "openai-compatible",
        "url": "http://127.0.0.1:9/v1", "options": {"preset": preset}});
    let mut v = server_less;
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    OpenAi::new(&contract::profile(v))
}

/// Speaks once per rate and checks each request against the operation.
fn requests_match(
    spec: &Fragment,
    preset: &str,
    extra: Value,
    voice: &str,
    rates: &[u32],
) -> Vec<Value> {
    let server = ScriptServer::start();
    server.on("/v1/audio/speech", Route::wav(&[3, -3], 24_000));
    let e = engine(profile(&server, preset, extra), Some(KEY));
    for &wpm in rates {
        speak(&e, "Contract test.", voice, wpm);
    }
    let seen = server.requests();
    assert_eq!(seen.len(), rates.len());
    seen.iter()
        .map(|req| {
            spec.assert_request("POST /v1/audio/speech", "", req, &[]);
            req.json()
        })
        .collect()
}

const RATES: &[u32] = &[100, 160, 200, 250, 333, 400];

// ---- OpenAI ----------------------------------------------------------

#[test]
fn openai_speech_requests_match_create_speech_request() {
    let spec = Fragment::load("openai");
    let server = ScriptServer::start();
    server.on("/v1/audio/speech", Route::wav(&[1, 2], 24_000));
    let e = engine(
        profile(
            &server,
            "openai",
            json!({"model": "gpt-4o-mini-tts", "voice": "coral", "key_ref": "credman",
                "options": {"instructions": "Speak calmly."}}),
        ),
        Some(KEY),
    );
    for &wpm in RATES {
        speak(&e, "Contract test.", "", wpm);
    }
    // A custom voice is an object with its id (the spec's Voice anyOf).
    speak(&e, "Contract test.", "voice_1234", 200);
    for req in server.requests() {
        spec.assert_request("POST /audio/speech", "/v1", &req, &[]);
        // securitySchemes ApiKeyAuth: http bearer.
        assert_eq!(
            req.header("authorization"),
            Some(format!("Bearer {KEY}").as_str())
        );
        let body = req.json();
        assert_eq!(body["response_format"], "wav");
    }
    let last = server.requests().pop().unwrap().json();
    assert_eq!(last["voice"], json!({"id": "voice_1234"}));
}

#[test]
fn openai_pcm_is_a_spec_format_read_at_24_khz() {
    let spec = Fragment::load("openai");
    let server = ScriptServer::start();
    server.on(
        "/v1/audio/speech",
        Route::new(200, "audio/pcm", pcm_bytes(&[7, -7, 7])),
    );
    let e = engine(
        profile(
            &server,
            "openai",
            json!({"model": "tts-1", "voice": "alloy", "key_ref": "credman",
                "options": {"response_format": "pcm"}}),
        ),
        Some(KEY),
    );
    let got = speak(&e, "Hi.", "", 200);
    assert_eq!(got[0].samples, vec![7, -7, 7]);
    assert_eq!(got[0].sample_rate, 24_000);
    let req = &server.requests()[0];
    spec.assert_request("POST /audio/speech", "/v1", req, &[]);
    assert_eq!(req.json()["response_format"], "pcm");
}

#[test]
fn openai_model_list_keeps_the_speech_models() {
    let spec = Fragment::load("openai");
    let list = json!({"object": "list", "data": [
        {"id": "gpt-4o-mini-tts", "object": "model", "created": 1742403959, "owned_by": "system"},
        {"id": "tts-1", "object": "model", "created": 1681940951, "owned_by": "openai-internal"},
        {"id": "gpt-5", "object": "model", "created": 1754425777, "owned_by": "system"},
        {"id": "whisper-1", "object": "model", "created": 1677532384, "owned_by": "openai-internal",
            "shutdown_date": null}
    ]});
    spec.assert_valid(
        &spec.response_schema("GET /models", 200),
        &list,
        Mode::Response,
        "model list example",
    );
    let server = ScriptServer::start();
    server.on("/v1/models", Route::json(200, &list.to_string()));
    let e = engine(
        profile(
            &server,
            "openai",
            json!({"model": "tts-1", "voice": "alloy", "key_ref": "credman"}),
        ),
        Some(KEY),
    );
    let ids: Vec<String> = e
        .refresh_models()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, vec!["gpt-4o-mini-tts", "tts-1"]);
    let req = &server.requests()[0];
    spec.assert_request("GET /models", "/v1", req, &[]);
    assert_eq!(
        req.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
}

#[test]
fn openai_documented_errors_map_to_reasons() {
    let spec = Fragment::load("openai");
    let op = "POST /audio/speech";
    let err = |kind: &str, code: Value, param: Value, message: &str| json!({"error": {"type": kind, "code": code, "param": param, "message": message}});
    let cases: Vec<(u16, Value, Reason)> = vec![
        (
            400,
            err(
                "invalid_request_error",
                Value::Null,
                json!("voice"),
                "Invalid voice 'nobody'.",
            ),
            Reason::BadVoice,
        ),
        (
            400,
            err(
                "invalid_request_error",
                Value::Null,
                json!("speed"),
                "Invalid speed.",
            ),
            Reason::BadConfig,
        ),
        (
            401,
            err(
                "invalid_request_error",
                json!("invalid_api_key"),
                Value::Null,
                "Incorrect API key provided.",
            ),
            Reason::Auth,
        ),
        (
            429,
            err(
                "requests",
                json!("rate_limit_exceeded"),
                Value::Null,
                "Rate limit reached.",
            ),
            Reason::RateLimited,
        ),
        (
            429,
            err(
                "insufficient_quota",
                json!("insufficient_quota"),
                Value::Null,
                "You exceeded your current quota.",
            ),
            Reason::Quota,
        ),
        (
            500,
            err(
                "server_error",
                Value::Null,
                Value::Null,
                "The voice could not be processed.",
            ),
            Reason::Server,
        ),
        (
            503,
            err(
                "server_error",
                json!("server_is_overloaded"),
                Value::Null,
                "Overloaded.",
            ),
            Reason::Server,
        ),
    ];
    let a = adapter(
        "openai",
        json!({"model": "gpt-4o-mini-tts", "voice": "coral"}),
    );
    for (status, body, want) in cases {
        assert!(spec.statuses(op).contains(&status), "{status} documented");
        spec.assert_valid(
            &spec.response_schema(op, status),
            &body,
            Mode::Response,
            &format!("{status} example"),
        );
        let e = a.map_error(&reply(status, &body), "coral", None);
        assert_eq!(e.reason, want, "{status} {body}");
        assert_eq!(e.status, Some(status));
    }
    // 403 is documented as text/plain carrying the JSON error text.
    let body = err(
        "invalid_request_error",
        json!("unsupported_country_region_territory"),
        Value::Null,
        "Country, region, or territory not supported",
    );
    let e = a.map_error(
        &contract::raw_reply(403, "text/plain", body.to_string().as_bytes()),
        "coral",
        None,
    );
    assert_eq!(e.reason, Reason::Auth);
    assert!(e.message.contains("Country, region"), "{}", e.message);
}

// ---- Kokoro-FastAPI --------------------------------------------------

#[test]
fn kokoro_fastapi_requests_match_its_speech_request() {
    let spec = Fragment::load("kokoro-fastapi");
    let bodies = requests_match(
        &spec,
        "kokoro-fastapi",
        json!({"voice": "af_heart"}),
        "",
        RATES,
    );
    for b in &bodies {
        assert_eq!(b["stream"], false);
        assert!(b.get("model").is_none(), "the server picks its own");
    }
    // A model, when the profile names one, is a plain string.
    requests_match(
        &spec,
        "kokoro-fastapi",
        json!({"voice": "af_heart", "model": "kokoro"}),
        "",
        &[250],
    );
}

#[test]
fn kokoro_fastapi_voice_lists_in_both_shapes() {
    let spec = Fragment::load("kokoro-fastapi");
    let op = "GET /v1/audio/voices";
    let current = json!({"voices": [
        {"id": "af_heart", "name": "af_heart", "target_quality": "A", "overall_grade": "A"},
        {"id": "am_adam", "name": "am_adam"}
    ], "default_voice": "af_heart"});
    let legacy = json!({"voices": ["af_heart", "am_adam"]});
    for list in [current, legacy] {
        spec.assert_valid(
            &spec.response_schema(op, 200),
            &list,
            Mode::Response,
            "voices",
        );
        let server = ScriptServer::start();
        server.on("/v1/audio/voices", Route::json(200, &list.to_string()));
        let e = engine(
            profile(&server, "kokoro-fastapi", json!({"voice": "af_heart"})),
            None,
        );
        let ids: Vec<String> = e
            .refresh_voices()
            .unwrap()
            .into_iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(ids, vec!["af_heart", "am_adam"]);
        spec.assert_request(op, "", &server.requests()[0], &[]);
    }
}

#[test]
fn kokoro_fastapi_errors_map_to_reasons() {
    let spec = Fragment::load("kokoro-fastapi");
    let op = "POST /v1/audio/speech";
    let a = adapter(
        "kokoro-fastapi",
        json!({"voice": "af_heart", "model": "tts-2"}),
    );
    let detail = |error: &str, message: &str, kind: &str| json!({"detail": {"error": error, "message": message, "type": kind}});
    let invalid_model = detail(
        "invalid_model",
        "Unsupported model: tts-2",
        "invalid_request_error",
    );
    spec.assert_valid(
        &spec.response_schema(op, 400),
        &invalid_model,
        Mode::Response,
        "400",
    );
    let e = a.map_error(&reply(400, &invalid_model), "af_heart", None);
    assert_eq!(e.reason, Reason::BadConfig);
    assert!(
        e.message.contains("does not know the model 'tts-2'"),
        "an unsupported model is named as the model: {}",
        e.message
    );
    let bad_voice = detail(
        "validation_error",
        "Voice 'zz_nobody' not found. Available voices: af_heart, am_adam",
        "invalid_request_error",
    );
    let e = a.map_error(&reply(400, &bad_voice), "zz_nobody", None);
    assert_eq!(e.reason, Reason::BadVoice);
    let server_error = detail("processing_error", "CUDA out of memory", "server_error");
    spec.assert_valid(
        &spec.response_schema(op, 500),
        &server_error,
        Mode::Response,
        "500",
    );
    let e = a.map_error(&reply(500, &server_error), "af_heart", None);
    assert_eq!(e.reason, Reason::Server);
    assert!(e.message.contains("CUDA out of memory"), "{}", e.message);
}

// ---- LocalAI ---------------------------------------------------------

#[test]
fn localai_requests_match_its_tts_request() {
    let spec = Fragment::load("localai");
    requests_match(
        &spec,
        "localai",
        json!({"model": "kokoro", "voice": "af_heart",
            "options": {"instructions": "Warm."}}),
        "",
        RATES,
    );
}

#[test]
fn localai_voice_and_model_lists() {
    let spec = Fragment::load("localai");
    let voices = json!({"data": [
        {"model": "kokoro", "voices": [
            {"name": "af_heart", "language": "en-us", "gender": "female"},
            {"name": "bf_emma", "language": "en-gb", "gender": "female"}]}
    ]});
    spec.assert_valid(
        &spec.response_schema("GET /v1/audio/voices", 200),
        &voices,
        Mode::Response,
        "voices",
    );
    let models = json!({"object": "list", "data": [
        {"id": "kokoro", "object": "model"}, {"id": "piper-en", "object": "model"}]});
    spec.assert_valid(
        &spec.response_schema("GET /v1/models", 200),
        &models,
        Mode::Response,
        "models",
    );
    let server = ScriptServer::start();
    server.on("/v1/audio/voices", Route::json(200, &voices.to_string()));
    server.on("/v1/models", Route::json(200, &models.to_string()));
    let e = engine(
        profile(
            &server,
            "localai",
            json!({"model": "kokoro", "voice": "af_heart", "key_ref": "credman"}),
        ),
        Some(KEY),
    );
    let got: Vec<(String, String)> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .map(|v| (v.id, v.language))
        .collect();
    assert_eq!(
        got,
        vec![
            ("af_heart".to_string(), "en-us".to_string()),
            ("bf_emma".to_string(), "en-gb".to_string())
        ]
    );
    let ids: Vec<String> = e
        .refresh_models()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, vec!["kokoro", "piper-en"]);
    for req in server.requests() {
        let op = if req.path.starts_with("/v1/models") {
            "GET /v1/models"
        } else {
            "GET /v1/audio/voices"
        };
        spec.assert_request(op, "", &req, &[]);
    }
}

#[test]
fn localai_documented_error_maps_to_a_reason() {
    let spec = Fragment::load("localai");
    let body = json!({"error": {"code": 404, "message": "voice not found for model kokoro",
        "type": "not_found"}});
    spec.assert_valid(
        &spec.response_schema("GET /v1/audio/voices", 404),
        &body,
        Mode::Response,
        "404",
    );
    let a = adapter("localai", json!({"model": "kokoro", "voice": "af_heart"}));
    assert_eq!(
        a.map_error(&reply(404, &body), "af_heart", None).reason,
        Reason::BadVoice
    );
}

// ---- Speaches --------------------------------------------------------

#[test]
fn speaches_requests_match_its_speech_request() {
    let spec = Fragment::load("speaches");
    let bodies = requests_match(
        &spec,
        "speaches",
        json!({"model": "speaches-ai/Kokoro-82M-v1.0-ONNX", "voice": "af_heart"}),
        "",
        RATES,
    );
    for b in &bodies {
        assert_eq!(b["sample_rate"], 24_000);
    }
    requests_match(
        &spec,
        "speaches",
        json!({"model": "speaches-ai/piper-en_US-amy-low", "voice": "amy",
            "options": {"sample_rate": 16000}}),
        "",
        &[250],
    );
}

#[test]
fn speaches_voice_and_model_lists() {
    let spec = Fragment::load("speaches");
    let voices = json!({"object": "list", "voices": [
        {"name": "af_heart", "language": "en-us", "gender": "female", "id": "af_heart"},
        {"name": "amy", "language": "en_US", "id": "amy"}
    ]});
    spec.assert_valid(
        &spec.response_schema("GET /v1/audio/voices", 200),
        &voices,
        Mode::Response,
        "voices",
    );
    let models = json!({"object": "list", "data": [
        {"id": "speaches-ai/Kokoro-82M-v1.0-ONNX", "created": 0, "object": "model",
            "owned_by": "speaches-ai", "language": ["en"], "task": "text-to-speech"},
        {"id": "Systran/faster-whisper-small", "created": 0, "object": "model",
            "owned_by": "Systran", "language": ["en"], "task": "automatic-speech-recognition"}
    ]});
    spec.assert_valid(
        &spec.response_schema("GET /v1/models", 200),
        &models,
        Mode::Response,
        "models",
    );
    let server = ScriptServer::start();
    server.on("/v1/audio/voices", Route::json(200, &voices.to_string()));
    server.on("/v1/models", Route::json(200, &models.to_string()));
    let e = engine(
        profile(
            &server,
            "speaches",
            json!({"model": "speaches-ai/Kokoro-82M-v1.0-ONNX", "voice": "af_heart"}),
        ),
        None,
    );
    let ids: Vec<String> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(ids, vec!["af_heart", "amy"]);
    let models: Vec<String> = e
        .refresh_models()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(models, vec!["speaches-ai/Kokoro-82M-v1.0-ONNX"]);
    for req in server.requests() {
        let op = if req.path.starts_with("/v1/models") {
            "GET /v1/models"
        } else {
            "GET /v1/audio/voices"
        };
        spec.assert_request(op, "", &req, &[]);
    }
}

#[test]
fn speaches_validation_error_maps_to_bad_config_naming_the_field() {
    let spec = Fragment::load("speaches");
    let body = json!({"detail": [{"loc": ["body", "sample_rate"],
        "msg": "Input should be less than or equal to 48000", "type": "less_than_equal"}]});
    spec.assert_valid(
        &spec.response_schema("POST /v1/audio/speech", 422),
        &body,
        Mode::Response,
        "422",
    );
    let a = adapter("speaches", json!({"model": "m", "voice": "af_heart"}));
    let e = a.map_error(&reply(422, &body), "af_heart", None);
    assert_eq!(e.reason, Reason::BadConfig);
    assert!(e.message.contains("body.sample_rate"), "{}", e.message);
}

// ---- openedai-speech -------------------------------------------------

#[test]
fn openedai_speech_requests_match_its_speech_request() {
    let spec = Fragment::load("openedai-speech");
    requests_match(
        &spec,
        "openedai-speech",
        json!({"voice": "alloy"}),
        "",
        RATES,
    );
    requests_match(
        &spec,
        "openedai-speech",
        json!({"voice": "alloy", "model": "tts-1-hd"}),
        "",
        &[250],
    );
}

#[test]
fn openedai_speech_pcm_rate_comes_from_its_content_type() {
    // speech.py: pcm is `audio/pcm;rate=22050` for tts-1 (no space).
    let server = ScriptServer::start();
    server.on(
        "/v1/audio/speech",
        Route::new(200, "audio/pcm;rate=22050", pcm_bytes(&[4, 5, 6])),
    );
    let e = engine(
        profile(
            &server,
            "openedai-speech",
            json!({"voice": "alloy", "model": "tts-1", "options": {"response_format": "pcm"}}),
        ),
        None,
    );
    let got = speak(&e, "Hi.", "", 200);
    assert_eq!(
        (got[0].samples.clone(), got[0].sample_rate),
        (vec![4, 5, 6], 22_050)
    );
    Fragment::load("openedai-speech").assert_request(
        "POST /v1/audio/speech",
        "",
        &server.requests()[0],
        &[],
    );
}

#[test]
fn openedai_speech_errors_map_to_reasons() {
    let spec = Fragment::load("openedai-speech");
    let op = "POST /v1/audio/speech";
    let a = adapter("openedai-speech", json!({"voice": "nobody"}));
    let voice = json!({"message": "Error loading voice: nobody, KeyError: 'nobody'",
        "code": 400, "type": "BadRequestError", "param": "voice"});
    spec.assert_valid(
        &spec.response_schema(op, 400),
        &voice,
        Mode::Response,
        "400",
    );
    assert_eq!(
        a.map_error(&reply(400, &voice), "nobody", None).reason,
        Reason::BadVoice
    );
    let busy = json!({"message": "Service unavailable, please try again later.",
        "code": 503, "type": "ServiceUnavailableError", "param": null});
    spec.assert_valid(&spec.response_schema(op, 503), &busy, Mode::Response, "503");
    assert_eq!(
        a.map_error(&reply(503, &busy), "alloy", None).reason,
        Reason::Server
    );
}

// ---- Chatterbox TTS API (travisvn) -----------------------------------

#[test]
fn chatterbox_api_requests_match_its_tts_request() {
    let spec = Fragment::load("chatterbox-api");
    requests_match(
        &spec,
        "chatterbox-api",
        json!({"voice": "narrator", "options": {"instructions": "never sent"}}),
        "",
        RATES,
    );
}

#[test]
fn chatterbox_api_long_text_stays_within_its_input_limit() {
    // TTSRequest.input: max_length 3000.
    let spec = Fragment::load("chatterbox-api");
    let server = ScriptServer::start();
    server.on("/v1/audio/speech", Route::wav(&[1], 24_000));
    let e = engine(
        profile(&server, "chatterbox-api", json!({"voice": "narrator"})),
        None,
    );
    let text = "This sentence is here to make the text long enough. ".repeat(90);
    speak(&e, text.trim(), "", 250);
    let seen = server.requests();
    assert!(seen.len() >= 2, "split into parts");
    for req in seen {
        spec.assert_request("POST /v1/audio/speech", "", &req, &[]);
    }
}

#[test]
fn chatterbox_api_voice_ids_are_the_names_the_speech_route_takes() {
    // The speech route resolves `voice` by library name or alias
    // (get_voice_path); a file name is unknown there and silently falls
    // back to the default voice. The list carries both.
    let spec = Fragment::load("chatterbox-api");
    let list = json!({"count": 2, "voices": [
        {"name": "narrator", "filename": "narrator.wav", "original_filename": "my narrator.wav",
            "file_extension": ".wav", "file_size": 512000, "upload_date": "2026-09-01T10:00:00",
            "path": "/app/voices/narrator.wav", "language": "en", "aliases": ["alloy"], "exists": true},
        {"name": "deutsch", "filename": "deutsch.mp3", "original_filename": "deutsch.mp3",
            "file_extension": ".mp3", "file_size": 256000, "upload_date": "2026-09-02T10:00:00",
            "path": "/app/voices/deutsch.mp3", "language": "de", "aliases": [], "exists": true}
    ]});
    spec.assert_valid(
        &spec.response_schema("GET /voices", 200),
        &list,
        Mode::Response,
        "voices",
    );
    let server = ScriptServer::start();
    server.on("/voices", Route::json(200, &list.to_string()));
    let e = engine(
        profile(&server, "chatterbox-api", json!({"voice": "narrator"})),
        None,
    );
    let got: Vec<(String, String)> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .map(|v| (v.id, v.language))
        .collect();
    assert_eq!(
        got,
        vec![
            ("narrator".to_string(), "en".to_string()),
            ("deutsch".to_string(), "de".to_string())
        ]
    );
    spec.assert_request("GET /voices", "", &server.requests()[0], &[]);
}

#[test]
fn chatterbox_api_error_body_is_read() {
    let spec = Fragment::load("chatterbox-api");
    let body =
        json!({"detail": {"error": {"message": "TTS model not loaded", "type": "model_error"}}});
    spec.assert_valid(
        &spec.response_schema("POST /v1/audio/speech", 500),
        &body,
        Mode::Response,
        "500",
    );
    let a = adapter("chatterbox-api", json!({"voice": "narrator"}));
    let e = a.map_error(&reply(500, &body), "narrator", None);
    assert_eq!(e.reason, Reason::Server);
    assert!(e.message.contains("TTS model not loaded"), "{}", e.message);
}

// ---- Chatterbox-TTS-Server (devnen) ----------------------------------

#[test]
fn chatterbox_server_requests_match_its_speech_request() {
    // OpenAISpeechRequest requires `model` (not read by the route): a
    // profile without one must still send a valid request.
    let spec = Fragment::load("chatterbox-server");
    let bodies = requests_match(
        &spec,
        "chatterbox-server",
        json!({"voice": "Emily.wav"}),
        "",
        RATES,
    );
    for b in &bodies {
        assert_eq!(b["response_format"], "wav");
    }
    requests_match(
        &spec,
        "chatterbox-server",
        json!({"voice": "Emily.wav", "model": "chatterbox", "options": {"response_format": "pcm"}}),
        "",
        &[250],
    );
}

#[test]
fn chatterbox_server_voice_ids_are_file_names() {
    let spec = Fragment::load("chatterbox-server");
    let list = json!([
        {"display_name": "Emily", "filename": "Emily.wav"},
        {"display_name": "Gianna", "filename": "Gianna.wav"}
    ]);
    spec.assert_valid(
        &spec.response_schema("GET /get_predefined_voices", 200),
        &list,
        Mode::Response,
        "voices",
    );
    let server = ScriptServer::start();
    server.on(
        "/get_predefined_voices",
        Route::json(200, &list.to_string()),
    );
    let e = engine(
        profile(&server, "chatterbox-server", json!({"voice": "Emily.wav"})),
        None,
    );
    let got: Vec<(String, String)> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .map(|v| (v.id, v.name))
        .collect();
    assert_eq!(
        got,
        vec![
            ("Emily.wav".to_string(), "Emily".to_string()),
            ("Gianna.wav".to_string(), "Gianna".to_string())
        ]
    );
    spec.assert_request("GET /get_predefined_voices", "", &server.requests()[0], &[]);
}

#[test]
fn chatterbox_server_errors_map_to_reasons() {
    let spec = Fragment::load("chatterbox-server");
    let op = "POST /v1/audio/speech";
    let a = adapter("chatterbox-server", json!({"voice": "Nobody.wav"}));
    let missing = json!({"detail": "Voice file 'Nobody.wav' not found."});
    spec.assert_valid(
        &spec.response_schema(op, 404),
        &missing,
        Mode::Response,
        "404",
    );
    assert_eq!(
        a.map_error(&reply(404, &missing), "Nobody.wav", None)
            .reason,
        Reason::BadVoice
    );
    let unloaded = json!({"detail": "TTS engine model is not currently loaded or available."});
    spec.assert_valid(
        &spec.response_schema(op, 503),
        &unloaded,
        Mode::Response,
        "503",
    );
    assert_eq!(
        a.map_error(&reply(503, &unloaded), "Emily.wav", None)
            .reason,
        Reason::Server
    );
    let invalid = json!({"detail": [{"loc": ["body", "model"], "msg": "Field required",
        "type": "missing"}]});
    spec.assert_valid(
        &spec.response_schema(op, 422),
        &invalid,
        Mode::Response,
        "422",
    );
    assert_eq!(
        a.map_error(&reply(422, &invalid), "Emily.wav", None).reason,
        Reason::BadConfig
    );
}
