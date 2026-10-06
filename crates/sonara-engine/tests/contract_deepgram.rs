//! Contract tests (#275) of kind `deepgram` against Deepgram's OpenAPI
//! (`tests/contracts/deepgram`): the `/v1/speak` query (model, encoding,
//! container, sample_rate, speed within 0.7 to 1.5) and body, the
//! `Authorization: Token` key, the `/v1/models` voice list, and the three
//! documented error body shapes. No real provider is called.
mod common;

use common::contract::{self, engine, pcm_bytes, reply, speak, Fragment, Mode};
use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::adapter::Adapter;
use sonara_engine::external::deepgram::{Deepgram, DEEPGRAM_RATES};
use sonara_engine::{Engine, Reason};

const KEY: &str = "dg-contract-0123456789";
const VOICE: &str = "aura-2-thalia-en";
const SPEAK: &str = "POST /v1/speak";

fn profile(server: &ScriptServer, options: Value) -> Value {
    let mut v = json!({"id": "dg", "kind": "deepgram", "url": server.base, "voice": VOICE,
        "send_mode": "sentence", "options": {"timeout_ms": 5000}});
    for (k, x) in options.as_object().unwrap() {
        v["options"][k] = x.clone();
    }
    v
}

#[test]
fn speak_requests_match_the_spec_at_every_rate_and_sample_rate() {
    let spec = Fragment::load("deepgram");
    for &rate in DEEPGRAM_RATES {
        let server = ScriptServer::start();
        server.on(
            "/v1/speak",
            Route::new(200, "audio/l16", pcm_bytes(&[8, -8])),
        );
        let e = engine(profile(&server, json!({"sample_rate": rate})), Some(KEY));
        for wpm in [100, 140, 200, 250, 300, 400] {
            let got = speak(&e, "Contract test.", "", wpm);
            assert_eq!(got[0].sample_rate, rate as u32);
        }
        for req in server.requests() {
            spec.assert_request(SPEAK, "", &req, &[]);
            // The variants that apply to raw linear16: encoding, no
            // container, a linear16 sample rate.
            let q = contract::parse_query(req.path.split_once('?').unwrap().1);
            let get = |k: &str| json!(q.iter().find(|(n, _)| n == k).unwrap().1.clone());
            for (name, variant) in [
                ("encoding", "V1SpeakPostParametersEncoding0"),
                ("container", "V1SpeakPostParametersContainer0"),
                ("sample_rate", "V1SpeakPostParametersSampleRate0"),
            ] {
                spec.assert_valid(&spec.def(variant), &get(name), Mode::Request, name);
            }
            // securitySchemes ApiKeyAuth: `Authorization: Token <API_KEY>`.
            assert_eq!(
                req.header("authorization"),
                Some(format!("Token {KEY}").as_str())
            );
        }
    }
}

#[test]
fn voice_list_is_the_tts_models() {
    let spec = Fragment::load("deepgram");
    let op = "GET /v1/models";
    let list = json!({
    "stt": [{"name": "nova-3", "canonical_name": "nova-3", "architecture": "nova-3",
        "languages": ["en"], "version": "2025-04-17.1", "uuid": "u1", "batch": true,
        "streaming": true, "formatted_output": true}],
    "tts": [
        {"name": "thalia", "canonical_name": "aura-2-thalia-en", "architecture": "aura-2",
            "languages": ["en", "en-US"], "version": "2025-04-07.0",
            "uuid": "ecb76e9d-f2db-4127-8060-79b05590d22f",
            "metadata": {"accent": "American", "age": "Adult", "color": "#C73D4C",
                "image": "https://static.deepgram.com/examples/avatars/thalia.jpg",
                "sample": "https://static.deepgram.com/examples/Aura-2-thalia.wav",
                "use_cases": ["Casual chat"]}},
        {"name": "helena", "canonical_name": "aura-2-helena-en", "architecture": "aura-2",
            "languages": ["en"], "version": "2025-04-07.0",
            "uuid": "0e6c8a1b-2b0b-4f5b-9d1f-3b3c3f1f8a11"}
    ]});
    spec.assert_valid(
        &spec.response_schema(op, 200),
        &list,
        Mode::Response,
        "models",
    );
    let server = ScriptServer::start();
    server.on("/v1/models", Route::json(200, &list.to_string()));
    let e = engine(profile(&server, json!({})), Some(KEY));
    let got: Vec<(String, String, String)> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .map(|v| (v.id, v.name, v.language))
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "aura-2-thalia-en".into(),
                "thalia (aura-2)".into(),
                "en".into()
            ),
            (
                "aura-2-helena-en".into(),
                "helena (aura-2)".into(),
                "en".into()
            )
        ]
    );
    let req = &server.requests()[0];
    spec.assert_request(op, "", req, &[]);
    assert_eq!(
        req.header("authorization"),
        Some(format!("Token {KEY}").as_str())
    );
}

#[test]
fn the_documented_error_shapes_map_to_reasons() {
    let spec = Fragment::load("deepgram");
    let schema = spec.response_schema(SPEAK, 400);
    let a = Deepgram::new(&contract::profile(
        json!({"id": "dg", "kind": "deepgram", "voice": VOICE}),
    ));
    let speed_legacy = json!({"err_code": "INVALID_QUERY_PARAMETER",
        "err_msg": "Failed to deserialize query parameters: speed is not supported for this model.",
        "request_id": "3d2f6a5e-0000-4000-8000-000000000001"});
    let model_modern = json!({"category": "INVALID_QUERY_PARAMETER",
        "message": "No such model/version/tier combination found.",
        "details": "model: aura-9-nobody-en", "request_id": "3d2f6a5e-0000-4000-8000-000000000002"});
    let text = json!("Bad Request: text is empty");
    for body in [&speed_legacy, &model_modern, &text] {
        spec.assert_valid(&schema, body, Mode::Response, "400");
    }
    let e = a.map_error(&reply(400, &speed_legacy), VOICE, None);
    assert_eq!(
        (e.reason, e.refused_param),
        (Reason::BadConfig, Some("speed"))
    );
    let e = a.map_error(&reply(400, &model_modern), VOICE, None);
    assert_eq!(e.reason, Reason::BadVoice);
    assert!(e.message.contains("No such model"), "{}", e.message);
    let e = a.map_error(
        &contract::raw_reply(400, "text/plain", b"Bad Request: text is empty"),
        VOICE,
        None,
    );
    assert_eq!(e.reason, Reason::BadConfig);
    assert!(e.message.contains("text is empty"), "{}", e.message);
    for (status, want) in [
        (401, Reason::Auth),
        (403, Reason::Auth),
        (429, Reason::RateLimited),
        (500, Reason::Server),
    ] {
        let body = json!({"err_code": "X", "err_msg": "m", "request_id": "r"});
        assert_eq!(a.map_error(&reply(status, &body), VOICE, None).reason, want);
    }
}
