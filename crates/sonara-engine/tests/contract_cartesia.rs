//! Contract tests (#275) of kind `cartesia` against Cartesia's API for
//! `Cartesia-Version: 2026-08-14` (`tests/contracts/cartesia`: the types
//! Cartesia's Python SDK generates from its OpenAPI, and the API
//! reference): the `/tts/bytes` body (model, voice object, raw
//! `pcm_s16le` output at every offered rate, `generation_config.speed`
//! within 0.6 to 1.5), the version and bearer headers, the cursor-paged
//! voice list in both documented voice shapes, and the status codes the
//! SDK names. No real provider is called.
mod common;

use common::contract::{self, engine, pcm_bytes, speak, Fragment, Mode};
use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::adapter::Adapter;
use sonara_engine::external::cartesia::{Cartesia, CARTESIA_RATES, CARTESIA_VERSION};
use sonara_engine::{Engine, Reason};

const KEY: &str = "sk_car_contract0123456789";
const VOICE: &str = "a0e99841-438c-4a64-b679-ae501e7d6091";

fn profile(server: &ScriptServer, options: Value) -> Value {
    let mut v = json!({"id": "ca", "kind": "cartesia", "url": server.base, "model": "sonic-3",
        "voice": VOICE, "send_mode": "sentence", "options": {"timeout_ms": 5000}});
    for (k, x) in options.as_object().unwrap() {
        v["options"][k] = x.clone();
    }
    v
}

fn headers_ok(req: &common::Captured) {
    assert_eq!(
        req.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(req.header("cartesia-version"), Some(CARTESIA_VERSION));
}

#[test]
fn tts_bytes_requests_match_the_spec_at_every_rate() {
    let spec = Fragment::load("cartesia");
    for &rate in CARTESIA_RATES {
        let server = ScriptServer::start();
        server.on(
            "/tts/bytes",
            Route::new(200, "audio/pcm", pcm_bytes(&[6, -6])),
        );
        let e = engine(
            profile(&server, json!({"sample_rate": rate, "language": "de"})),
            Some(KEY),
        );
        for wpm in [100, 150, 200, 250, 300, 400] {
            let got = speak(&e, "Contract test.", "", wpm);
            assert_eq!(got[0].sample_rate, rate as u32);
        }
        for req in server.requests() {
            spec.assert_request("POST /tts/bytes", "", &req, &[]);
            headers_ok(&req);
        }
    }
}

#[test]
fn the_voice_list_reads_both_documented_voice_shapes() {
    let spec = Fragment::load("cartesia");
    let op = "GET /voices";
    // API reference: accents[] {accent, locale, is_native}.
    let page_1 = json!({"data": [
        {"id": "v-1", "name": "Katie", "tagline": "Friendly", "description": "A warm voice",
            "gender": "feminine", "language": "en", "is_owner": false, "access": "public",
            "status": "active", "created_at": "2026-01-01T00:00:00Z",
            "accents": [{"accent": "general-american", "locale": "en-US", "is_native": true}]}
    ], "has_more": true, "next_page": "v-1"});
    // SDK types: locales[] {locale, is_native}, accent a catalog id.
    let page_2 = json!({"data": [
        {"id": "v-2", "name": "Ellis", "tagline": "", "description": "A calm voice",
            "gender": "masculine", "language": "en", "is_owner": true, "access": "private",
            "created_at": "2026-02-01T00:00:00Z", "accent": "british",
            "locales": [{"locale": "en-GB", "is_native": true}, {"locale": "fr-FR", "is_native": false}]}
    ], "has_more": false, "next_page": null});
    for page in [&page_1, &page_2] {
        spec.assert_valid(&spec.response_schema(op, 200), page, Mode::Response, "page");
    }
    let server = ScriptServer::start();
    server.queue("/voices", Route::json(200, &page_1.to_string()));
    server.queue("/voices", Route::json(200, &page_2.to_string()));
    let e = engine(profile(&server, json!({})), Some(KEY));
    let got: Vec<(String, String)> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .filter(|v| v.id != VOICE)
        .map(|v| (v.id, v.language))
        .collect();
    assert_eq!(
        got,
        vec![
            ("v-1".into(), "en-US".into()),
            ("v-2".into(), "en-GB".into())
        ]
    );
    let seen = server.requests();
    assert_eq!(seen.len(), 2);
    for req in &seen {
        spec.assert_request(op, "", req, &[]);
        headers_ok(req);
    }
    assert!(
        seen[1].path.contains("starting_after=v-1"),
        "{}",
        seen[1].path
    );
}

#[test]
fn the_sdk_status_codes_map_to_reasons() {
    let spec = Fragment::load("cartesia");
    let a = Cartesia::new(&contract::profile(json!({"id": "ca", "kind": "cartesia",
        "model": "sonic-3", "voice": VOICE})));
    let cases = [
        (
            400,
            json!({"title": "Invalid request", "message": "transcript is empty"}),
            Reason::BadConfig,
        ),
        (
            401,
            json!({"title": "Unauthorized", "message": "Invalid API key"}),
            Reason::Auth,
        ),
        (
            403,
            json!({"title": "Forbidden", "message": "Key lacks permission"}),
            Reason::Auth,
        ),
        (
            404,
            json!({"title": "Not found", "message": "Voice not found"}),
            Reason::BadVoice,
        ),
        (
            422,
            json!({"title": "Unprocessable", "message": "output_format is invalid"}),
            Reason::BadConfig,
        ),
        (
            429,
            json!({"title": "Too many requests", "message": "Rate limited"}),
            Reason::RateLimited,
        ),
        (
            500,
            json!({"title": "Internal error", "message": "Try again"}),
            Reason::Server,
        ),
    ];
    for (status, body, want) in cases {
        assert!(
            spec.statuses("POST /tts/bytes").contains(&status),
            "{status}"
        );
        let e = a.map_error(&contract::reply(status, &body), VOICE, None);
        assert_eq!(e.reason, want, "{status} {body}");
    }
}
