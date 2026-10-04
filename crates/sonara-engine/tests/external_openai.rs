//! Kind `openai-compatible` against a scripted local server standing in for
//! OpenAI and the local servers (spec 13.1): request bodies and headers per
//! preset, audio formats, voice lists. No real provider is called.
mod common;

use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig};
use sonara_engine::{Engine, Error, Reason};
use std::sync::Arc;

fn engine(server: &ScriptServer, v: Value, key: Option<&str>) -> External {
    let mut v = v;
    if v.get("url").is_none() {
        v["url"] = json!(format!("{}/v1", server.base));
    }
    v["id"] = json!("oa");
    v["kind"] = json!("openai-compatible");
    let store = Arc::new(MemoryStore::new());
    if let Some(k) = key {
        store.set("oa", &Secret::new(k)).unwrap();
    }
    let profile = Profile::from_json(&v).unwrap();
    External::new(ExternalConfig::new(profile, KeyResolver::new(store))).unwrap()
}

fn speak(e: &External, text: &str, voice: &str, rate: u32) -> Vec<i16> {
    e.synthesize(text, voice, rate)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .flat_map(|c| c.samples)
        .collect()
}

#[test]
fn request_body_golden_per_preset() {
    let cases: Vec<(Value, &str, u32, Value)> = vec![
        (
            json!({"key_ref": "credman", "options": {"preset": "openai", "instructions": "Warm."}}),
            "",
            250,
            json!({"model": "gpt-4o-mini-tts", "input": "Hi.", "voice": "marin",
                "response_format": "wav", "speed": 1.25, "instructions": "Warm."}),
        ),
        (
            json!({"options": {"preset": "kokoro-fastapi"}}),
            "af_bella",
            200,
            json!({"model": "kokoro", "input": "Hi.", "voice": "af_bella",
                "response_format": "wav", "speed": 1.0, "stream": false}),
        ),
        (
            json!({"model": "speaches-ai/Kokoro-82M-v1.0-ONNX", "options": {"preset": "speaches"}}),
            "",
            100,
            json!({"model": "speaches-ai/Kokoro-82M-v1.0-ONNX", "input": "Hi.", "voice": "af_heart",
                "response_format": "wav", "speed": 0.5, "sample_rate": 24000}),
        ),
        (
            json!({"model": "kokoro", "voice": "af_sky", "options": {"preset": "localai"}}),
            "",
            400,
            json!({"model": "kokoro", "input": "Hi.", "voice": "af_sky",
                "response_format": "wav", "speed": 2.0}),
        ),
        (
            json!({"options": {"preset": "generic", "extra": {"lang_code": "a", "speed": 1.1}}}),
            "",
            200,
            json!({"model": "tts-1", "input": "Hi.", "voice": "alloy",
                "response_format": "wav", "speed": 1.1, "lang_code": "a"}),
        ),
    ];
    for (profile, voice, rate, want) in cases {
        let server = ScriptServer::start();
        server.on("/v1/audio/speech", Route::wav(&[1], 24_000));
        let e = engine(&server, profile.clone(), Some("sk-x"));
        speak(&e, "Hi.", voice, rate);
        let req = &server.requests()[0];
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/v1/audio/speech");
        assert_eq!(req.json(), want, "{profile}");
        assert_eq!(req.header("content-type"), Some("application/json"));
    }
}

#[test]
fn authorization_is_sent_only_with_a_key() {
    let server = ScriptServer::start();
    server.on("/v1/audio/speech", Route::wav(&[1], 24_000));
    let with = engine(&server, json!({"key_ref": "credman"}), Some("sk-local-123"));
    speak(&with, "One.", "", 200);
    let without = engine(&server, json!({}), Some("sk-local-123"));
    speak(&without, "Two.", "", 200);
    let seen = server.requests();
    assert_eq!(seen[0].header("authorization"), Some("Bearer sk-local-123"));
    assert_eq!(seen[1].header("authorization"), None, "key_ref none");
}

#[test]
fn raw_pcm_uses_the_content_type_rate() {
    let server = ScriptServer::start();
    let body: Vec<u8> = [10i16, 20, 30]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    server.on(
        "/v1/audio/speech",
        Route::new(200, "audio/pcm; rate=22050", body),
    );
    let e = engine(
        &server,
        json!({"options": {"preset": "generic", "response_format": "pcm"}}),
        None,
    );
    let chunks: Vec<_> = e
        .synthesize("Hi.", "", 200)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(chunks[0].samples, vec![10, 20, 30]);
    assert_eq!(chunks[0].sample_rate, 22_050);
    assert_eq!(server.requests()[0].json()["response_format"], "pcm");
}

#[test]
fn an_mp3_answer_is_a_format_failure() {
    let server = ScriptServer::start();
    server.on(
        "/v1/audio/speech",
        Route::new(200, "audio/mpeg", b"ID3\x04\0\0\0\0\0\0".to_vec()),
    );
    let e = engine(&server, json!({}), None);
    match e.test("Hi.", "", 200) {
        Err(Error::External { reason, message }) => {
            assert_eq!(reason, Reason::Format);
            assert!(message.contains("MP3"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn voices_per_preset() {
    let server = ScriptServer::start();
    let list = |preset: &str, extra: Value| {
        let mut v = json!({"model": "m", "voice": "default.wav", "options": {"preset": preset}});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        let e = engine(&server, v, None);
        e.refresh_voices()
            .unwrap()
            .into_iter()
            .map(|v| v.id)
            .collect::<Vec<_>>()
    };
    // Kokoro-FastAPI, both shapes.
    server.queue(
        "/v1/audio/voices",
        Route::json(200, r#"{"voices": ["af_heart", "am_adam"]}"#),
    );
    assert_eq!(
        list("kokoro-fastapi", json!({"voice": "af_heart"})),
        vec!["af_heart", "am_adam"]
    );
    server.queue(
        "/v1/audio/voices",
        Route::json(200, r#"{"voices": [{"id": "bf_emma", "name": "Emma"}]}"#),
    );
    assert_eq!(
        list("kokoro-fastapi", json!({"voice": "bf_emma"})),
        vec!["bf_emma"]
    );
    // LocalAI: the model in the query.
    server.queue(
        "/v1/audio/voices",
        Route::json(
            200,
            r#"{"data": [{"model": "m", "voices": [{"name": "v1", "language": "en"}]}]}"#,
        ),
    );
    assert_eq!(list("localai", json!({"voice": "v1"})), vec!["v1"]);
    assert_eq!(
        server.requests().last().unwrap().path,
        "/v1/audio/voices?model=m"
    );
    // Speaches.
    server.queue(
        "/v1/audio/voices",
        Route::json(
            200,
            r#"{"voices": [{"id": "af_heart", "name": "Heart", "language": "en-us"}]}"#,
        ),
    );
    assert_eq!(
        list("speaches", json!({"voice": "af_heart"})),
        vec!["af_heart"]
    );
    // Chatterbox API: under the root, not /v1.
    server.queue(
        "/voices",
        Route::json(
            200,
            r#"{"voices": [{"name": "default.wav", "aliases": [], "language": "en"}]}"#,
        ),
    );
    assert_eq!(list("chatterbox-api", json!({})), vec!["default.wav"]);
    // Chatterbox-TTS-Server.
    server.queue(
        "/get_predefined_voices",
        Route::json(
            200,
            r#"[{"display_name": "Emily", "filename": "Emily.wav"}, {"display_name": "Default", "filename": "default.wav"}]"#,
        ),
    );
    assert_eq!(
        list("chatterbox-server", json!({})),
        vec!["Emily.wav", "default.wav"]
    );
    // Generic: a 404 is an empty list (plus the profile's voice).
    assert_eq!(list("generic", json!({})), vec!["default.wav"]);
}

#[test]
fn a_failed_voice_list_is_an_error_except_for_generic() {
    let server = ScriptServer::start();
    server.on(
        "/v1/audio/voices",
        Route::json(500, r#"{"detail": "down"}"#),
    );
    let e = engine(
        &server,
        json!({"options": {"preset": "kokoro-fastapi"}}),
        None,
    );
    assert!(matches!(
        e.refresh_voices(),
        Err(Error::External {
            reason: Reason::Server,
            ..
        })
    ));
    // The cached list stays usable (the profile voice at least).
    assert_eq!(e.voices()[0].id, "af_heart");
    let g = engine(&server, json!({"options": {"preset": "generic"}}), None);
    assert_eq!(g.refresh_voices().unwrap().len(), 1);
}

#[test]
fn openai_voices_are_fixed_and_need_no_request() {
    let server = ScriptServer::start();
    let e = engine(
        &server,
        json!({"url": "https://api.openai.com/v1", "options": {"preset": "openai"}}),
        None,
    );
    let ids: Vec<String> = e.voices().into_iter().map(|v| v.id).collect();
    assert_eq!(ids.len(), 13);
    assert!(ids.contains(&"marin".to_string()));
    assert_eq!(e.refresh_voices().unwrap().len(), 13);
    assert!(server.requests().is_empty());
    assert_eq!(e.lookahead(), 2, "a cloud profile prefetches two");
}
