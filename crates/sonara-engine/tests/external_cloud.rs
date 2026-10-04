//! Kinds `elevenlabs`, `azure` and `google` (spec 5.4, 13.1, 13.2) against a
//! scripted local server standing in for each provider: the request each
//! sends (path, query, auth header, body), the audio each answers, voice
//! lists (ElevenLabs paged), and failures that speak with the fallback. No
//! real provider is called.
mod common;

use base64::Engine as _;
use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::error::cue_text;
use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, Error, Readiness, Reason};
use std::sync::Arc;

const KEY: &str = "cloud-test-key-0123456789";

fn engine(server: &ScriptServer, mut v: Value, key: Option<&str>, fallback: bool) -> External {
    v["url"] = json!(server.base);
    let id = v["id"].as_str().unwrap().to_string();
    let store = Arc::new(MemoryStore::new());
    if let Some(k) = key {
        store.set(&id, &Secret::new(k)).unwrap();
    }
    let mut config = ExternalConfig::new(Profile::from_json(&v).unwrap(), KeyResolver::new(store));
    if fallback {
        config.fallback = Some(Arc::new(FakeEngine::new()) as Arc<dyn Engine>);
    }
    External::new(config).unwrap()
}

fn chunks(e: &External, text: &str, voice: &str, rate: u32) -> Vec<sonara_engine::PcmChunk> {
    e.synthesize(text, voice, rate)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn elevenlabs() -> Value {
    json!({"id": "el", "kind": "elevenlabs", "voice": "JBFqnCBsd6RMkjVDRZzb",
        "options": {"timeout_ms": 5000}})
}

fn azure() -> Value {
    json!({"id": "az", "kind": "azure", "voice": "en-US-AvaMultilingualNeural",
        "options": {"timeout_ms": 5000}})
}

fn google() -> Value {
    json!({"id": "gg", "kind": "google", "voice": "en-US-Chirp3-HD-Kore",
        "options": {"timeout_ms": 5000}})
}

#[test]
fn elevenlabs_speaks_raw_pcm_with_its_key_header() {
    let server = ScriptServer::start();
    let path = "/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb";
    server.on(path, Route::new(200, "audio/pcm", pcm_bytes(&[5, -5, 9])));
    let e = engine(&server, elevenlabs(), Some(KEY), false);
    let got = chunks(&e, "Hello there.", "", 250);
    assert_eq!(got[0].samples, vec![5, -5, 9]);
    assert_eq!(got[0].sample_rate, 24_000);
    let req = &server.requests()[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, format!("{path}?output_format=pcm_24000"));
    assert_eq!(req.header("xi-api-key"), Some(KEY));
    assert_eq!(req.header("authorization"), None);
    assert_eq!(
        req.json(),
        json!({"text": "Hello there.", "model_id": "eleven_flash_v2_5",
            "voice_settings": {"speed": 1.2}})
    );
    // Another voice id (a cloned voice) goes in the path as given.
    server.on(
        "/v1/text-to-speech/myClonedVoice1",
        Route::new(200, "audio/pcm", pcm_bytes(&[1])),
    );
    chunks(&e, "Again.", "myClonedVoice1", 200);
    assert_eq!(
        server.requests()[1].path,
        "/v1/text-to-speech/myClonedVoice1?output_format=pcm_24000"
    );
    assert_eq!(e.lookahead(), 1, "a loopback url prefetches one");
}

#[test]
fn elevenlabs_voice_list_follows_three_pages() {
    let server = ScriptServer::start();
    for page in [
        r#"{"voices": [{"voice_id": "a", "name": "A", "labels": {"language": "en"}}],
            "has_more": true, "next_page_token": "p2"}"#,
        r#"{"voices": [{"voice_id": "b", "name": "B"}], "has_more": true, "next_page_token": "p3"}"#,
        r#"{"voices": [{"voice_id": "c", "name": "C"}, {"voice_id": "a", "name": "A"}],
            "has_more": false, "next_page_token": null}"#,
    ] {
        server.queue("/v2/voices", Route::json(200, page));
    }
    let e = engine(&server, elevenlabs(), Some(KEY), false);
    let voices = e.refresh_voices().unwrap();
    let ids: Vec<&str> = voices.iter().map(|v| v.id.as_str()).collect();
    // The profile's voice first (not listed), then the three pages, once each.
    assert_eq!(ids, vec!["JBFqnCBsd6RMkjVDRZzb", "a", "b", "c"]);
    assert_eq!(voices[1].language, "en");
    let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        vec![
            "/v2/voices?page_size=100",
            "/v2/voices?page_size=100&next_page_token=p2",
            "/v2/voices?page_size=100&next_page_token=p3",
        ]
    );
    assert!(server
        .requests()
        .iter()
        .all(|r| r.header("xi-api-key") == Some(KEY)));
}

#[test]
fn elevenlabs_out_of_credit_speaks_with_the_fallback_and_the_cue() {
    let server = ScriptServer::start();
    server.on(
        "/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb",
        Route::json(
            402,
            r#"{"detail": {"code": "insufficient_credits", "message": "Not enough credits."}}"#,
        ),
    );
    let e = engine(&server, elevenlabs(), Some(KEY), true);
    let got: Vec<i16> = chunks(&e, "One.", "", 200)
        .into_iter()
        .flat_map(|c| c.samples)
        .collect();
    let cue = cue_text(Reason::Quota, "ElevenLabs");
    assert_eq!(
        cue,
        "ElevenLabs is out of credit. Reading with the built-in voice."
    );
    let mut want = FakeEngine::render(&cue, "", 200).unwrap();
    want.extend(FakeEngine::render("One.", "", 200).unwrap());
    assert_eq!(got, want);
    let st = e.status();
    assert_eq!(st.reason, Some(Reason::Quota));
    assert_eq!(st.readiness, Readiness::Waiting);
    assert!(st.message.unwrap().contains("Not enough credits."));
}

#[test]
fn azure_sends_ssml_with_its_headers() {
    let server = ScriptServer::start();
    server.on(
        "/cognitiveservices/v1",
        Route::new(200, "audio/x-wav", pcm_bytes(&[1, 2])),
    );
    let e = engine(&server, azure(), Some(KEY), false);
    let got = chunks(&e, "Fish & chips <now>.", "", 200);
    assert_eq!(
        (got[0].samples.clone(), got[0].sample_rate),
        (vec![1, 2], 24_000)
    );
    let req = &server.requests()[0];
    assert_eq!(req.path, "/cognitiveservices/v1");
    assert_eq!(req.header("ocp-apim-subscription-key"), Some(KEY));
    assert_eq!(req.header("content-type"), Some("application/ssml+xml"));
    assert_eq!(
        req.header("x-microsoft-outputformat"),
        Some("raw-24khz-16bit-mono-pcm")
    );
    assert!(req.header("user-agent").unwrap().starts_with("Sonara/"));
    assert_eq!(
        String::from_utf8(req.body.clone()).unwrap(),
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
         <voice name='en-US-AvaMultilingualNeural'><prosody rate='1'>\
         Fish &amp; chips &lt;now&gt;.</prosody></voice></speak>"
    );
}

#[test]
fn azure_unknown_voice_is_bad_voice_once_the_list_is_known() {
    let server = ScriptServer::start();
    server.on(
        "/cognitiveservices/voices/list",
        Route::json(
            200,
            r#"[{"ShortName": "en-US-AvaMultilingualNeural", "LocalName": "Ava", "Locale": "en-US"}]"#,
        ),
    );
    server.on("/cognitiveservices/v1", Route::new(400, "text/plain", ""));
    let e = engine(&server, azure(), Some(KEY), false);
    // No list yet: a 400 is a settings problem.
    assert!(matches!(
        e.test("x", "en-US-Nobody", 200),
        Err(Error::External {
            reason: Reason::BadConfig,
            ..
        })
    ));
    let voices = e.refresh_voices().unwrap();
    assert_eq!(voices[0].name, "Ava (en-US)");
    assert_eq!(
        server
            .requests()
            .last()
            .unwrap()
            .header("ocp-apim-subscription-key"),
        Some(KEY)
    );
    match e.test("x", "en-US-Nobody", 200) {
        Err(Error::External { reason, message }) => {
            assert_eq!(reason, Reason::BadVoice);
            assert!(
                message.contains("'en-US-Nobody' is not in the voice list"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
    // A listed voice that still gets a 400 stays a settings problem.
    assert!(matches!(
        e.test("x", "en-US-AvaMultilingualNeural", 200),
        Err(Error::External {
            reason: Reason::BadConfig,
            ..
        })
    ));
}

#[test]
fn azure_refused_key_names_the_region() {
    let server = ScriptServer::start();
    server.on("/cognitiveservices/v1", Route::new(401, "text/plain", ""));
    let e = engine(&server, azure(), Some(KEY), false);
    match e.test("x", "", 200) {
        Err(Error::External { reason, message }) => {
            assert_eq!(reason, Reason::Auth);
            assert!(message.contains("region"), "{message}");
            assert!(!message.contains(KEY));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn google_decodes_base64_audio_and_sends_the_key_in_a_header() {
    let server = ScriptServer::start();
    let b64 = base64::engine::general_purpose::STANDARD.encode(pcm_bytes(&[11, 22, 33]));
    server.on(
        "/v1/text:synthesize",
        Route::json(200, &json!({"audioContent": b64}).to_string()),
    );
    let e = engine(&server, google(), Some(KEY), false);
    let got = chunks(&e, "Hello.", "", 300);
    assert_eq!(
        (got[0].samples.clone(), got[0].sample_rate),
        (vec![11, 22, 33], 24_000)
    );
    let req = &server.requests()[0];
    assert_eq!(req.path, "/v1/text:synthesize");
    assert_eq!(req.header("x-goog-api-key"), Some(KEY));
    assert!(!req.path.contains("key"));
    assert_eq!(
        req.json(),
        json!({"input": {"text": "Hello."},
            "voice": {"languageCode": "en-US", "name": "en-US-Chirp3-HD-Kore"},
            "audioConfig": {"audioEncoding": "PCM", "sampleRateHertz": 24000,
                "speakingRate": 1.5}})
    );
}

#[test]
fn google_splits_by_utf8_bytes() {
    let server = ScriptServer::start();
    let b64 = base64::engine::general_purpose::STANDARD.encode(pcm_bytes(&[1]));
    server.on(
        "/v1/text:synthesize",
        Route::json(200, &json!({"audioContent": b64}).to_string()),
    );
    let e = engine(&server, google(), Some(KEY), false);
    // 2000 three-byte characters in words: 6000+ bytes, under 5000 chars.
    let text = vec!["日本語の文"; 400].join(" ");
    assert!(text.chars().count() < 5000 && text.len() > 5000);
    let got = chunks(&e, &text, "", 200);
    assert_eq!(got.len(), 2);
    let sent: Vec<String> = server
        .requests()
        .iter()
        .map(|r| r.json()["input"]["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|t| t.len() <= 5000));
    assert_eq!(sent.join(" "), text);
}

#[test]
fn google_invalid_key_is_auth() {
    let server = ScriptServer::start();
    server.on(
        "/v1/text:synthesize",
        Route::json(
            400,
            r#"{"error": {"code": 400, "message": "API key not valid. Please pass a valid API key.",
                "status": "INVALID_ARGUMENT", "details": [{"reason": "API_KEY_INVALID"}]}}"#,
        ),
    );
    server.on(
        "/v1/voices",
        Route::json(
            200,
            r#"{"voices": [{"languageCodes": ["en-US"], "name": "en-US-Chirp3-HD-Kore"}]}"#,
        ),
    );
    let e = engine(&server, google(), Some(KEY), true);
    match e.test("x", "", 200) {
        Err(Error::External { reason, message }) => {
            assert_eq!(reason, Reason::Auth);
            assert!(
                message.starts_with("Google Text-to-Speech refused the key (400)"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
    let ids: Vec<String> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(ids, vec!["en-US-Chirp3-HD-Kore"]);
}

#[test]
fn cloud_kinds_without_a_key_never_send_text() {
    for profile in [elevenlabs(), azure(), google()] {
        let server = ScriptServer::start();
        let e = engine(&server, profile.clone(), None, true);
        assert!(e.warm().is_ok(), "a fallback exists");
        chunks(&e, "Private text.", "", 200);
        assert!(server.requests().is_empty(), "{profile}");
        assert_eq!(e.status().reason, Some(Reason::NoKey), "{profile}");
    }
}

/// Near-silent neural TTS often starts with sample -1 (bytes FF FF), which
/// looks like an MP3 frame sync. Raw PCM that the adapter asked for is
/// never sniffed as MP3: each kind speaks it, and stays usable.
#[test]
fn cloud_pcm_that_starts_with_minus_one_is_spoken() {
    let quiet = [-1i16, 0, -2, 7];
    let server = ScriptServer::start();
    server.on(
        "/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb",
        Route::new(200, "audio/pcm", pcm_bytes(&quiet)),
    );
    server.on(
        "/cognitiveservices/v1",
        Route::new(200, "audio/x-wav", pcm_bytes(&quiet)),
    );
    let b64 = base64::engine::general_purpose::STANDARD.encode(pcm_bytes(&quiet));
    server.on(
        "/v1/text:synthesize",
        Route::json(200, &json!({"audioContent": b64}).to_string()),
    );
    for v in [elevenlabs(), azure(), google()] {
        let kind = v["kind"].as_str().unwrap().to_string();
        let e = engine(&server, v, Some(KEY), false);
        for _ in 0..2 {
            let got = chunks(&e, "Hello.", "", 200);
            assert_eq!(got[0].samples, quiet.to_vec(), "{kind}");
        }
        assert_eq!(e.status().readiness, Readiness::Ready, "{kind}");
    }
}
