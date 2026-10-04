//! Kind `openai-compatible` against a scripted local server standing in for
//! OpenAI and the local servers (spec 13.1): request bodies and headers per
//! preset, audio formats, voice and model lists, and no model or voice in
//! code (#235). No real provider is called.
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
    let profile = Profile::from_json(&v).unwrap();
    if let Some(k) = key {
        store
            .set("oa", &Secret::new(k), &profile.origin().unwrap())
            .unwrap();
    }
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
            json!({"model": "m1", "voice": "v1", "key_ref": "credman",
                "options": {"preset": "openai", "instructions": "Warm."}}),
            "",
            250,
            json!({"model": "m1", "input": "Hi.", "voice": "v1",
                "response_format": "wav", "speed": 1.25, "instructions": "Warm."}),
        ),
        // No model named: none sent, the server picks its own (#235).
        (
            json!({"options": {"preset": "kokoro-fastapi"}}),
            "v2",
            200,
            json!({"input": "Hi.", "voice": "v2",
                "response_format": "wav", "speed": 1.0, "stream": false}),
        ),
        (
            json!({"model": "org/m-2", "voice": "v1", "options": {"preset": "speaches"}}),
            "",
            100,
            json!({"model": "org/m-2", "input": "Hi.", "voice": "v1",
                "response_format": "wav", "speed": 0.5, "sample_rate": 24000}),
        ),
        (
            json!({"model": "m3", "voice": "v3", "options": {"preset": "localai"}}),
            "",
            400,
            json!({"model": "m3", "input": "Hi.", "voice": "v3",
                "response_format": "wav", "speed": 2.0}),
        ),
        (
            json!({"voice": "v1",
                "options": {"preset": "generic", "extra": {"lang_code": "a", "speed": 1.1}}}),
            "",
            200,
            json!({"input": "Hi.", "voice": "v1",
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
    speak(&with, "One.", "v1", 200);
    let without = engine(&server, json!({}), Some("sk-local-123"));
    speak(&without, "Two.", "v1", 200);
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
        .synthesize("Hi.", "v1", 200)
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
    match e.test("Hi.", "v1", 200) {
        Err(Error::External { reason, message }) => {
            assert_eq!(reason, Reason::Format);
            assert!(message.contains("MP3"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_localai_voice_list_carries_the_key() {
    // The query of the voices URL must not keep the key off the request
    // (review of #224: key_allowed reparsed the whole URL and refused '?').
    let server = ScriptServer::start();
    server.queue(
        "/v1/audio/voices",
        Route::json(
            200,
            r#"{"data": [{"model": "m", "voices": [{"name": "v1"}]}]}"#,
        ),
    );
    let e = engine(
        &server,
        json!({"model": "m", "voice": "v1", "key_ref": "credman", "options": {"preset": "localai"}}),
        Some("sk-local-9"),
    );
    e.refresh_voices().unwrap();
    let seen = server.requests();
    assert_eq!(seen[0].path, "/v1/audio/voices?model=m");
    assert_eq!(seen[0].header("authorization"), Some("Bearer sk-local-9"));
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
        json!({"voice": "v1", "options": {"preset": "kokoro-fastapi"}}),
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
    assert_eq!(e.voices()[0].id, "v1");
    let g = engine(
        &server,
        json!({"voice": "v1", "options": {"preset": "generic"}}),
        None,
    );
    assert_eq!(g.refresh_voices().unwrap().len(), 1);
}

#[test]
fn openai_has_no_voice_list_in_code() {
    // No voice list API: no voice is offered but the profile's (#235), and
    // nothing is fetched.
    let server = ScriptServer::start();
    let e = engine(
        &server,
        json!({"url": "https://api.openai.com/v1", "options": {"preset": "openai"}}),
        None,
    );
    assert!(e.voices().is_empty());
    assert!(e.refresh_voices().unwrap().is_empty());
    assert!(server.requests().is_empty());
    assert_eq!(e.lookahead(), 2, "a cloud profile prefetches two");
    // OpenAI needs a model: without one it says so.
    let st = e.status();
    assert_eq!(st.reason, Some(Reason::BadConfig));
    assert_eq!(
        st.message.as_deref(),
        Some("Choose a model for OpenAI in Sonara's settings (Engines).")
    );
    let named = engine(
        &server,
        json!({"url": "https://api.openai.com/v1", "model": "m1", "voice": "v1",
            "options": {"preset": "openai"}}),
        None,
    );
    let ids: Vec<String> = named.voices().into_iter().map(|v| v.id).collect();
    assert_eq!(ids, vec!["v1"], "the profile's own voice");
    assert_eq!(named.status().reason, None);
}

#[test]
fn models_come_from_the_server_list() {
    let server = ScriptServer::start();
    server.on(
        "/v1/models",
        Route::json(
            200,
            r#"{"object": "list", "data": [{"id": "x-1", "task": "automatic-speech-recognition"},
                {"id": "speak-1", "task": "text-to-speech"}]}"#,
        ),
    );
    let e = engine(
        &server,
        json!({"key_ref": "credman", "options": {"preset": "speaches"}}),
        Some("sk-m"),
    );
    assert!(e.has_model_list());
    let ids: Vec<String> = e
        .refresh_models()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, vec!["speak-1"]);
    let seen = server.requests();
    assert_eq!(seen[0].path, "/v1/models");
    assert_eq!(seen[0].header("authorization"), Some("Bearer sk-m"));
    // A server without the list: empty, the model is typed in.
    let bare = ScriptServer::start();
    let k = engine(
        &bare,
        json!({"options": {"preset": "kokoro-fastapi"}}),
        None,
    );
    assert!(k.refresh_models().unwrap().is_empty());
    // The profile's own model is always offered.
    let own = engine(
        &bare,
        json!({"model": "mine", "options": {"preset": "kokoro-fastapi"}}),
        None,
    );
    let ids: Vec<String> = own
        .refresh_models()
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, vec!["mine"]);
}

#[test]
fn an_unknown_model_names_it() {
    let server = ScriptServer::start();
    server.on(
        "/v1/audio/speech",
        Route::json(
            404,
            r#"{"error": {"message": "The model `gone-1` does not exist", "type": "invalid_request_error", "code": "model_not_found"}}"#,
        ),
    );
    let e = engine(
        &server,
        json!({"model": "gone-1", "voice": "v1", "options": {"preset": "generic"}}),
        None,
    );
    match e.test("Hi.", "", 200) {
        Err(Error::External { reason, message }) => {
            assert_eq!(reason, Reason::BadConfig);
            assert!(
                message.starts_with("The speech server does not know the model 'gone-1' (404)"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
}
