//! Contract tests (#275) of kind `elevenlabs` against ElevenLabs' OpenAPI
//! (`tests/contracts/elevenlabs`): the whole and `/stream` speech requests
//! (path, `output_format` and `enable_logging` query, `xi-api-key` header,
//! body against `Body_text_to_speech_*`), the paged `/v2/voices` and the
//! `/v1/models` list fed with spec-shaped answers, and the documented 422.
//! No real provider is called.
mod common;

use common::contract::{self, engine, pcm_bytes, reply, speak, Fragment, Mode};
use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::adapter::Adapter;
use sonara_engine::external::elevenlabs::{ElevenLabs, ELEVENLABS_FORMATS};
use sonara_engine::{Engine, Reason};
use std::time::Duration;

const KEY: &str = "xi-contract-0123456789";
const VOICE: &str = "21m00Tcm4TlvDq8ikWAM";
const WHOLE: &str = "POST /v1/text-to-speech/{voice_id}";
const STREAM: &str = "POST /v1/text-to-speech/{voice_id}/stream";

fn profile(server: &ScriptServer, mode: &str, options: Value) -> Value {
    let mut v = json!({"id": "el", "kind": "elevenlabs", "url": server.base,
        "voice": VOICE, "send_mode": mode, "options": {"timeout_ms": 5000}});
    for (k, x) in options.as_object().unwrap() {
        if k == "model" {
            v["model"] = x.clone();
        } else {
            v["options"][k] = x.clone();
        }
    }
    v
}

fn check(spec: &Fragment, req: &common::Captured) {
    let op = if req.path.contains("/stream") {
        STREAM
    } else {
        WHOLE
    };
    let params = spec.assert_request(op, "", req, &[]);
    assert_eq!(params, vec![("voice_id".to_string(), VOICE.to_string())]);
    // The key is the documented `xi-api-key` header parameter, never
    // a bearer token.
    assert_eq!(req.header("xi-api-key"), Some(KEY));
    assert_eq!(req.header("authorization"), None);
}

#[test]
fn whole_speech_requests_match_the_spec() {
    let spec = Fragment::load("elevenlabs");
    let cases = [
        json!({}),
        json!({"model": "eleven_multilingual_v2", "stability": 0.4,
            "similarity_boost": 0.8, "style": 0.2, "language_code": "en"}),
        json!({"output_format": "pcm_44100", "enable_logging": false}),
        json!({"output_format": "pcm_16000"}),
    ];
    for options in cases {
        let server = ScriptServer::start();
        server.on(
            &format!("/v1/text-to-speech/{VOICE}"),
            Route::new(200, "audio/pcm", pcm_bytes(&[2, -2])),
        );
        let e = engine(profile(&server, "sentence", options.clone()), Some(KEY));
        for wpm in [100, 200, 250, 400] {
            speak(&e, "Contract test.", "", wpm);
        }
        for req in server.requests() {
            check(&spec, &req);
        }
    }
}

#[test]
fn every_format_sonara_offers_is_a_spec_output_format() {
    let spec = Fragment::load("elevenlabs");
    for op in [WHOLE, STREAM] {
        let p = spec
            .params(op)
            .into_iter()
            .find(|p| p.name == "output_format")
            .unwrap();
        for f in ELEVENLABS_FORMATS {
            spec.assert_valid(&p.schema, &json!(f), Mode::Request, op);
        }
    }
}

#[test]
fn stream_requests_match_the_spec_and_play_as_they_arrive() {
    let spec = Fragment::load("elevenlabs");
    let server = ScriptServer::start();
    server.on(
        &format!("/v1/text-to-speech/{VOICE}/stream"),
        Route::raw(
            "audio/pcm",
            vec![
                (Duration::ZERO, pcm_bytes(&[1, 2, 3])),
                (Duration::from_millis(20), pcm_bytes(&[4, 5])),
            ],
        ),
    );
    let e = engine(
        profile(&server, "message", json!({"model": "eleven_flash_v2_5"})),
        Some(KEY),
    );
    let samples: Vec<i16> = speak(&e, "First sentence. Second sentence.", "", 250)
        .into_iter()
        .flat_map(|c| c.samples)
        .collect();
    assert_eq!(samples, vec![1, 2, 3, 4, 5]);
    let seen = server.requests();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].path.contains("/stream?"), "{}", seen[0].path);
    check(&spec, &seen[0]);
}

#[test]
fn paged_voice_list_follows_the_spec_cursor() {
    let spec = Fragment::load("elevenlabs");
    let op = "GET /v2/voices";
    let voice = |id: &str, name: &str, language: &str| {
        json!({"voice_id": id, "name": name, "category": "premade",
            "labels": {"accent": "american", "language": language},
            "available_for_tiers": [], "high_quality_base_model_ids": [],
            "settings": null, "sharing": null, "description": null})
    };
    let first = json!({"voices": [voice("v1", "Rachel", "en"), voice("v2", "Hans", "de")],
        "has_more": true, "total_count": 3, "next_page_token": "tok 2"});
    let last = json!({"voices": [voice("v3", "Sara", "")],
        "has_more": false, "total_count": 3, "next_page_token": null});
    for page in [&first, &last] {
        spec.assert_valid(&spec.response_schema(op, 200), page, Mode::Response, "page");
    }
    let server = ScriptServer::start();
    server.queue("/v2/voices", Route::json(200, &first.to_string()));
    server.queue("/v2/voices", Route::json(200, &last.to_string()));
    let e = engine(profile(&server, "sentence", json!({})), Some(KEY));
    let got: Vec<(String, String, String)> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .filter(|v| v.id != VOICE)
        .map(|v| (v.id, v.name, v.language))
        .collect();
    assert_eq!(
        got,
        vec![
            ("v1".into(), "Rachel".into(), "en".into()),
            ("v2".into(), "Hans".into(), "de".into()),
            ("v3".into(), "Sara".into(), "".into())
        ]
    );
    let seen = server.requests();
    assert_eq!(seen.len(), 2);
    for req in &seen {
        spec.assert_request(op, "", req, &[]);
        assert_eq!(req.header("xi-api-key"), Some(KEY));
        let q = contract::parse_query(req.path.split_once('?').unwrap().1);
        let size: u32 = q
            .iter()
            .find(|(k, _)| k == "page_size")
            .unwrap()
            .1
            .parse()
            .unwrap();
        assert!(size <= 100, "page_size 'can not exceed 100'");
    }
    assert!(
        seen[1].path.contains("next_page_token=tok%202"),
        "{}",
        seen[1].path
    );
}

#[test]
fn model_list_keeps_the_text_to_speech_models() {
    let spec = Fragment::load("elevenlabs");
    let op = "GET /v1/models";
    let model = |id: &str, name: &str, tts: bool| {
        json!({"model_id": id, "name": name, "can_be_finetuned": false,
            "can_do_text_to_speech": tts, "can_do_voice_conversion": !tts,
            "can_use_style": tts, "can_use_speaker_boost": true, "serves_pro_voices": false,
            "token_cost_factor": 1.0, "description": "", "requires_alpha_access": false,
            "max_characters_request_free_user": 2500,
            "max_characters_request_subscribed_user": 10000,
            "maximum_text_length_per_request": 10000,
            "languages": [{"language_id": "en", "name": "English"}],
            "model_rates": {"character_cost_multiplier": 1.0}, "concurrency_group": "standard"})
    };
    let models = json!([
        model("eleven_multilingual_v2", "Eleven Multilingual v2", true),
        model("eleven_english_sts_v2", "Eleven English v2", false)
    ]);
    spec.assert_valid(
        &spec.response_schema(op, 200),
        &models,
        Mode::Response,
        "models",
    );
    let server = ScriptServer::start();
    server.on("/v1/models", Route::json(200, &models.to_string()));
    let e = engine(profile(&server, "sentence", json!({})), Some(KEY));
    let got: Vec<(String, String)> = e
        .refresh_models()
        .unwrap()
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();
    assert_eq!(
        got,
        vec![(
            "eleven_multilingual_v2".into(),
            "Eleven Multilingual v2".into()
        )]
    );
    spec.assert_request(op, "", &server.requests()[0], &[]);
}

#[test]
fn the_documented_validation_error_is_a_settings_problem() {
    let spec = Fragment::load("elevenlabs");
    let body = json!({"detail": [{"loc": ["query", "output_format"],
        "msg": "Input should be 'mp3_22050_32', 'pcm_16000' or 'pcm_24000'",
        "type": "enum"}]});
    for op in [WHOLE, STREAM] {
        spec.assert_valid(&spec.response_schema(op, 422), &body, Mode::Response, "422");
    }
    let a = ElevenLabs::new(&contract::profile(json!({"id": "el", "kind": "elevenlabs",
        "voice": VOICE, "options": {"output_format": "pcm_44100"}})));
    let e = a.map_error(&reply(422, &body), VOICE, None);
    assert_eq!(e.reason, Reason::BadConfig);
    assert!(e.message.contains("query.output_format"), "{}", e.message);
}
