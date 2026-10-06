//! Contract tests (#275) of kinds `google` (Cloud Text-to-Speech) and
//! `gemini` against Google's discovery documents (`tests/contracts/google`,
//! `tests/contracts/gemini`) and the google.rpc.Status error model
//! (`tests/contracts/google-rpc`): request bodies validated strictly
//! (Google refuses unknown fields), the key only in the `x-goog-api-key`
//! header, spec-shaped audio, voice and model lists, and documented error
//! bodies through the adapters' error mapping. No real provider is called.
mod common;

use base64::Engine as _;
use common::contract::{self, captured, engine, reply, rpc_error, speak, Fragment, Mode};
use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::adapter::Adapter;
use sonara_engine::external::gemini::Gemini;
use sonara_engine::external::google::Google;
use sonara_engine::external::keys::Secret;
use sonara_engine::{Engine, Reason};
use std::time::Duration;

const KEY: &str = "AIzaSyContractKey0123456789abcdefghijk";

fn b64(samples: &[i16]) -> String {
    base64::engine::general_purpose::STANDARD.encode(contract::pcm_bytes(samples))
}

fn no_key_in_the_url(req: &common::Captured) {
    let q = contract::parse_query(req.path.split_once('?').map_or("", |(_, q)| q));
    assert!(q.iter().all(|(k, _)| k != "key"), "{}", req.path);
    assert_eq!(req.header("x-goog-api-key"), Some(KEY));
}

/// Every error body below is a google.rpc.Status: checked first.
fn rpc(code: u16, status: &str, message: &str, details: Value) -> Value {
    let body = rpc_error(code, status, message, details);
    let spec = Fragment::load("google-rpc");
    spec.assert_valid(&spec.def("ErrorResponse"), &body, Mode::Response, "error");
    body
}

// ---- Cloud Text-to-Speech ---------------------------------------------

fn google(server: &ScriptServer, options: Value) -> Value {
    let mut v = json!({"id": "gg", "kind": "google", "url": server.base,
        "voice": "en-US-Chirp3-HD-Charon", "send_mode": "sentence",
        "options": {"timeout_ms": 5000}});
    for (k, x) in options.as_object().unwrap() {
        v["options"][k] = x.clone();
    }
    v
}

#[test]
fn google_synthesize_requests_match_the_discovery_document() {
    let spec = Fragment::load("google");
    let answer = json!({"audioContent": b64(&[9, -9, 9])});
    spec.assert_valid(
        &spec.response_schema("text.synthesize", 200),
        &answer,
        Mode::Response,
        "answer",
    );
    for options in [
        json!({}),
        json!({"language_code": "en-GB", "sample_rate": 16000}),
        json!({"model_name": "chirp-3-hd"}),
    ] {
        let server = ScriptServer::start();
        server.on("/v1/text:synthesize", Route::json(200, &answer.to_string()));
        let e = engine(google(&server, options), Some(KEY));
        for wpm in [100, 200, 250, 400] {
            let got = speak(&e, "Contract test.", "", wpm);
            assert_eq!(got[0].samples, vec![9, -9, 9]);
        }
        for req in server.requests() {
            spec.assert_request("text.synthesize", "", &req, &[]);
            no_key_in_the_url(&req);
            assert_eq!(req.json()["audioConfig"]["audioEncoding"], "PCM");
        }
    }
}

#[test]
fn google_linear16_answers_carry_a_wav_header_and_are_read() {
    // AudioEncoding LINEAR16: "Audio content returned as LINEAR16 also
    // contains a WAV header."
    let pcm = sonara_engine::PcmChunk {
        samples: vec![5, 6, 7],
        sample_rate: 22_050,
        channels: 1,
    };
    let wav = sonara_engine::wav::encode(&pcm);
    let body = json!({"audioContent": base64::engine::general_purpose::STANDARD.encode(wav)});
    let a = Google::new(&contract::profile(json!({"id": "gg", "kind": "google",
        "voice": "en-US-Standard-A"})));
    let got = a
        .audio(&reply(200, &body), "Google")
        .expect("a WAV in audioContent");
    assert_eq!((got.samples, got.sample_rate), (vec![5, 6, 7], 22_050));
}

#[test]
fn google_voice_list_matches_the_discovery_document() {
    let spec = Fragment::load("google");
    let list = json!({"voices": [
        {"languageCodes": ["en-US"], "name": "en-US-Chirp3-HD-Charon", "ssmlGender": "MALE",
            "naturalSampleRateHertz": 24000},
        {"languageCodes": ["de-DE"], "name": "de-DE-Neural2-A", "ssmlGender": "FEMALE",
            "naturalSampleRateHertz": 24000}
    ]});
    spec.assert_valid(
        &spec.response_schema("voices.list", 200),
        &list,
        Mode::Response,
        "voices",
    );
    let server = ScriptServer::start();
    server.on("/v1/voices", Route::json(200, &list.to_string()));
    let e = engine(
        google(&server, json!({"language_code": "en-US"})),
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
            ("en-US-Chirp3-HD-Charon".into(), "en-US".into()),
            ("de-DE-Neural2-A".into(), "de-DE".into())
        ]
    );
    let req = &server.requests()[0];
    spec.assert_request("voices.list", "", req, &[]);
    no_key_in_the_url(req);
}

#[test]
fn google_rpc_errors_map_to_reasons() {
    let a = Google::new(&contract::profile(json!({"id": "gg", "kind": "google",
        "voice": "en-US-Nobody"})));
    let key_invalid = rpc(
        400,
        "INVALID_ARGUMENT",
        "API key not valid. Please pass a valid API key.",
        json!([{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "API_KEY_INVALID",
            "domain": "googleapis.com", "metadata": {"service": "texttospeech.googleapis.com"}}]),
    );
    let cases = [
        (400, key_invalid, Reason::Auth),
        (
            400,
            rpc(
                400,
                "INVALID_ARGUMENT",
                "Voice 'en-US-Nobody' does not exist. Is it misspelled?",
                json!([]),
            ),
            Reason::BadVoice,
        ),
        (
            400,
            rpc(
                400,
                "INVALID_ARGUMENT",
                "This voice does not support speaking rate or pitch parameters at this time.",
                json!([]),
            ),
            Reason::BadConfig,
        ),
        (
            403,
            rpc(
                403,
                "PERMISSION_DENIED",
                "Cloud Text-to-Speech API has not been used in project 1 before or it is disabled.",
                json!([{"@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": "SERVICE_DISABLED", "domain": "googleapis.com"}]),
            ),
            Reason::Auth,
        ),
        (
            429,
            rpc(
                429,
                "RESOURCE_EXHAUSTED",
                "Quota exceeded for quota metric 'Requests' and limit 'Requests per minute'.",
                json!([{"@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": "RATE_LIMIT_EXCEEDED", "domain": "googleapis.com"}]),
            ),
            Reason::RateLimited,
        ),
        (
            500,
            rpc(500, "INTERNAL", "Internal error encountered.", json!([])),
            Reason::Server,
        ),
        (
            503,
            rpc(
                503,
                "UNAVAILABLE",
                "The service is currently unavailable.",
                json!([]),
            ),
            Reason::Server,
        ),
    ];
    for (status, body, want) in cases {
        let e = a.map_error(&reply(status, &body), "en-US-Nobody", None);
        assert_eq!(e.reason, want, "{status} {body}");
    }
}

// ---- Gemini -----------------------------------------------------------

const MODEL: &str = "gemini-2.5-flash-preview-tts";

fn gemini_profile(server: &ScriptServer, voice: &str, options: Value) -> Value {
    let mut v = json!({"id": "ge", "kind": "gemini", "url": server.base, "model": MODEL,
        "voice": voice, "options": {"timeout_ms": 5000}});
    for (k, x) in options.as_object().unwrap() {
        v["options"][k] = x.clone();
    }
    v
}

fn audio_event(samples: &[i16], finish: Option<&str>) -> Value {
    let mut candidate = json!({"content": {"role": "model", "parts": [
        {"inlineData": {"mimeType": "audio/L16;codec=pcm;rate=24000", "data": b64(samples)}}]},
        "index": 0});
    if let Some(f) = finish {
        candidate["finishReason"] = json!(f);
    }
    json!({"candidates": [candidate], "modelVersion": MODEL, "responseId": "r1"})
}

#[test]
fn gemini_stream_requests_match_the_discovery_document() {
    let spec = Fragment::load("gemini");
    let events = [audio_event(&[1, 2], None), audio_event(&[3], Some("STOP"))];
    for ev in &events {
        spec.assert_valid(
            &spec.response_schema("models.streamGenerateContent", 200),
            ev,
            Mode::Response,
            "event",
        );
    }
    for (voice, options) in [
        ("Kore", json!({})),
        (
            "voice_abc123",
            json!({"style": "warm and calm", "language_code": "de-DE"}),
        ),
        ("voicekey_xyz", json!({})),
    ] {
        let server = ScriptServer::start();
        let path = format!("/v1beta/models/{MODEL}:streamGenerateContent");
        server.on(
            &path,
            Route::sse(
                events
                    .iter()
                    .map(|e| (Duration::ZERO, e.to_string()))
                    .collect(),
            ),
        );
        let e = engine(gemini_profile(&server, voice, options), Some(KEY));
        for wpm in [120, 250, 300, 400] {
            let got: Vec<i16> = speak(&e, "Contract test.", "", wpm)
                .into_iter()
                .flat_map(|c| c.samples)
                .collect();
            assert_eq!(got, vec![1, 2, 3]);
        }
        for req in server.requests() {
            // `alt=sse` is the documented way to stream
            // (ai.google.dev/api/generate-content); the discovery
            // document's `alt` enum lists only json, media and proto.
            let params = spec.assert_request("models.streamGenerateContent", "", &req, &["alt"]);
            assert_eq!(params, vec![("modelsId".to_string(), MODEL.to_string())]);
            assert!(req.path.ends_with("?alt=sse"), "{}", req.path);
            no_key_in_the_url(&req);
        }
    }
}

#[test]
fn gemini_whole_answer_request_matches_the_discovery_document() {
    let spec = Fragment::load("gemini");
    let a = Gemini::new(&contract::profile(json!({"id": "ge", "kind": "gemini",
        "model": MODEL, "voice": "Kore", "options": {"style": "cheerful"}})));
    for wpm in [100, 250, 400] {
        let req =
            captured(&a.synth_request("Contract test.", "Kore", wpm, Some(&Secret::new(KEY))));
        spec.assert_request("models.generateContent", "", &req, &[]);
        no_key_in_the_url(&req);
    }
    let whole = audio_event(&[4, 5], Some("STOP"));
    spec.assert_valid(
        &spec.response_schema("models.generateContent", 200),
        &whole,
        Mode::Response,
        "answer",
    );
    let got = a.audio(&reply(200, &whole), "Gemini").unwrap();
    assert_eq!((got.samples, got.sample_rate), (vec![4, 5], 24_000));
}

#[test]
fn gemini_blocked_prompt_is_no_audio_with_its_reason() {
    let spec = Fragment::load("gemini");
    let blocked = json!({"promptFeedback": {"blockReason": "PROHIBITED_CONTENT"}});
    spec.assert_valid(
        &spec.response_schema("models.generateContent", 200),
        &blocked,
        Mode::Response,
        "blocked",
    );
    let a = Gemini::new(&contract::profile(json!({"id": "ge", "kind": "gemini",
        "model": MODEL, "voice": "Kore"})));
    let e = a.audio(&reply(200, &blocked), "Gemini").unwrap_err();
    assert_eq!(e.reason, Reason::Server);
    assert!(e.message.contains("PROHIBITED_CONTENT"), "{}", e.message);
}

#[test]
fn gemini_model_and_voice_lists_match_the_spec() {
    let spec = Fragment::load("gemini");
    let models = json!({"models": [
        {"name": "models/gemini-2.5-flash-preview-tts", "baseModelId": "gemini-2.5-flash-preview-tts",
            "displayName": "Gemini 2.5 Flash Preview TTS",
            "version": "gemini-2.5-flash-exp-tts-2025-05-19", "inputTokenLimit": 8192,
            "outputTokenLimit": 16384, "supportedGenerationMethods": ["countTokens", "generateContent"]},
        {"name": "models/gemini-2.5-flash", "baseModelId": "gemini-2.5-flash", "version": "001",
            "displayName": "Gemini 2.5 Flash",
            "supportedGenerationMethods": ["generateContent"]}
    ], "nextPageToken": ""});
    spec.assert_valid(
        &spec.response_schema("models.list", 200),
        &models,
        Mode::Response,
        "models",
    );
    let voices_1 = json!({"voices": [
        {"id": "voice_abc123", "display_name": "My voice", "language_code": "en-US",
            "type": "STORED", "description": "Cloned from a sample"}
    ], "next_page_token": "p2"});
    let voices_2 = json!({"voices": [
        {"id": "Kore", "display_name": "Kore", "description": "Firm", "language_code": "en-US",
            "gender": "FEMALE", "type": "PREBUILT"}
    ]});
    for v in [&voices_1, &voices_2] {
        spec.assert_valid(
            &spec.response_schema("voices.list", 200),
            v,
            Mode::Response,
            "voices",
        );
    }
    let server = ScriptServer::start();
    server.on("/v1beta/models", Route::json(200, &models.to_string()));
    server.queue("/v1beta/voices", Route::json(200, &voices_1.to_string()));
    server.queue("/v1beta/voices", Route::json(200, &voices_2.to_string()));
    let e = engine(gemini_profile(&server, "Kore", json!({})), Some(KEY));
    let model_ids: Vec<String> = e
        .refresh_models()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(model_ids, vec![MODEL]);
    let voice_ids: Vec<String> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(voice_ids, vec!["voice_abc123", "Kore"]);
    for req in server.requests() {
        let op = if req.path.starts_with("/v1beta/models") {
            "models.list"
        } else {
            "voices.list"
        };
        spec.assert_request(op, "", &req, &[]);
        no_key_in_the_url(&req);
    }
}

#[test]
fn gemini_rpc_errors_map_to_reasons_and_waits() {
    let a = Gemini::new(&contract::profile(json!({"id": "ge", "kind": "gemini",
        "model": MODEL, "voice": "Kore"})));
    let quota = |id: &str| {
        rpc(
            429,
            "RESOURCE_EXHAUSTED",
            "You exceeded your current quota, please check your plan and billing details.",
            json!([
                {"@type": "type.googleapis.com/google.rpc.QuotaFailure", "violations": [
                    {"quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_requests",
                     "quotaId": id, "quotaDimensions": {"model": MODEL, "location": "global"},
                     "quotaValue": "15"}]},
                {"@type": "type.googleapis.com/google.rpc.Help", "links": [
                    {"description": "Learn more", "url": "https://ai.google.dev/gemini-api/docs/rate-limits"}]},
                {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "39s"}
            ]),
        )
    };
    let per_minute = a.map_error(
        &reply(
            429,
            &quota("GenerateRequestsPerMinutePerProjectPerModel-FreeTier"),
        ),
        "Kore",
        None,
    );
    assert_eq!(per_minute.reason, Reason::RateLimited);
    assert_eq!(per_minute.retry_after, Some(Duration::from_secs(39)));
    let per_day = a.map_error(
        &reply(
            429,
            &quota("GenerateRequestsPerDayPerProjectPerModel-FreeTier"),
        ),
        "Kore",
        None,
    );
    assert_eq!(per_day.reason, Reason::Quota);
    assert!(per_day.retry_after.unwrap() >= Duration::from_secs(3600));
    let cases = [
        (
            400,
            rpc(
                400,
                "INVALID_ARGUMENT",
                "API key not valid. Please pass a valid API key.",
                json!([{"@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": "API_KEY_INVALID", "domain": "googleapis.com"}]),
            ),
            Reason::Auth,
        ),
        (
            403,
            rpc(403, "PERMISSION_DENIED", "Method doesn't allow unregistered callers.", json!([])),
            Reason::Auth,
        ),
        (
            404,
            rpc(
                404,
                "NOT_FOUND",
                "models/gemini-old-tts is not found for API version v1beta, or is not supported for generateContent.",
                json!([]),
            ),
            Reason::BadConfig,
        ),
        (
            500,
            rpc(500, "INTERNAL", "An internal error has occurred.", json!([])),
            Reason::Server,
        ),
        (
            503,
            rpc(503, "UNAVAILABLE", "The model is overloaded. Please try again later.", json!([])),
            Reason::Server,
        ),
    ];
    for (status, body, want) in cases {
        let e = a.map_error(&reply(status, &body), "Kore", None);
        assert_eq!(e.reason, want, "{status} {body}");
    }
    let unknown_field = rpc(
        400,
        "INVALID_ARGUMENT",
        "Invalid JSON payload received. Unknown name \"responseFormat\" at 'generation_config': Cannot find field.",
        json!([{"@type": "type.googleapis.com/google.rpc.BadRequest", "fieldViolations": [
            {"field": "generation_config", "description": "Invalid JSON payload received. Unknown name \"responseFormat\" at 'generation_config': Cannot find field."}]}]),
    );
    let e = a.map_error(&reply(400, &unknown_field), "Kore", None);
    assert_eq!(
        (e.reason, e.refused_param),
        (Reason::BadConfig, Some("responseFormat"))
    );
}
