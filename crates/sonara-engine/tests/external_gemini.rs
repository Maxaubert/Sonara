//! Kind `gemini` (#235) against a scripted local server standing in for
//! the Gemini API: the request (path, key header, body), the base64 PCM it
//! answers, the fixed voice list, failures that speak with the fallback
//! (a refused key, an unknown voice, a 429 with its retryDelay), the key
//! bound to Google's address, no redirects, and nothing sent while muted.
//! The real Gemini API is never called.
mod common;

use base64::Engine as _;
use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::error::cue_text;
use sonara_engine::external::hold::Hold;
use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, Error, Readiness, Reason};
use std::sync::Arc;

const KEY: &str = "AIzaSyGeminiTestKey0123456789abcdefghi";
const SPEAK: &str = "/v1beta/models/gemini-3.8-flash-lite-tts:generateContent";
const GOOGLE: &str = "https://generativelanguage.googleapis.com:443";

fn gemini() -> Value {
    json!({"id": "ge", "kind": "gemini", "options": {"timeout_ms": 5000}})
}

struct Rig {
    server: ScriptServer,
    engine: External,
    fake: Arc<FakeEngine>,
}

fn rig_with(v: Value, key: Option<&str>, fallback: bool, hold: Option<Arc<Hold>>) -> Rig {
    let server = ScriptServer::start();
    let mut v = v;
    v["url"] = json!(server.base);
    let store = Arc::new(MemoryStore::new());
    let profile = Profile::from_json(&v).unwrap();
    if let Some(k) = key {
        store
            .set("ge", &Secret::new(k), &profile.origin().unwrap())
            .unwrap();
    }
    let fake = Arc::new(FakeEngine::new());
    let mut config = ExternalConfig::new(profile, KeyResolver::new(store));
    if fallback {
        config.fallback = Some(fake.clone() as Arc<dyn Engine>);
    }
    config.hold = hold;
    Rig {
        engine: External::new(config).unwrap(),
        server,
        fake,
    }
}

fn rig() -> Rig {
    rig_with(gemini(), Some(KEY), true, None)
}

fn audio(samples: &[i16]) -> Route {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Route::json(
        200,
        &json!({"candidates": [{"content": {"role": "model", "parts": [{"inlineData":
            {"mimeType": "audio/L16;codec=pcm;rate=24000", "data": data}}]},
            "finishReason": "STOP"}]})
        .to_string(),
    )
}

fn rpc_error(code: u16, status: &str, message: &str, details: Value) -> Route {
    Route::json(
        code,
        &json!({"error": {"code": code, "message": message, "status": status,
            "details": details}})
        .to_string(),
    )
}

fn samples(e: &External, text: &str, voice: &str, rate: u32) -> Vec<i16> {
    e.synthesize(text, voice, rate)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .flat_map(|c| c.samples)
        .collect()
}

fn with_cue(reason: Reason, text: &str) -> Vec<i16> {
    let mut want = FakeEngine::render(&cue_text(reason, "Gemini"), "", 200).unwrap();
    want.extend(FakeEngine::render(text, "", 200).unwrap());
    want
}

/// Whether any request carried a key header or the key.
fn saw_a_key(server: &ScriptServer) -> bool {
    server.requests().iter().any(|r| {
        r.path.contains("key=")
            || r.headers
                .iter()
                .any(|(k, v)| k == "x-goog-api-key" || k == "authorization" || v.contains(KEY))
    })
}

#[test]
fn gemini_speaks_base64_pcm_with_the_key_only_in_its_header() {
    let r = rig();
    r.server.on(SPEAK, audio(&[5, -5, 9]));
    let got = r
        .engine
        .synthesize("Hello there.", "", 200)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(got[0].samples, vec![5, -5, 9]);
    assert_eq!(got[0].sample_rate, 24_000);
    let req = &r.server.requests()[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, SPEAK, "no key and no query in the URL");
    assert_eq!(req.header("x-goog-api-key"), Some(KEY));
    assert_eq!(req.header("authorization"), None);
    assert_eq!(
        req.json(),
        json!({"contents": [{"role": "user", "parts": [{"text": "Hello there."}]}],
            "generationConfig": {"responseModalities": ["AUDIO"],
                "speechConfig": {"voiceConfig": {"prebuiltVoiceConfig": {"voiceName": "Kore"}}},
                "responseFormat": {"audio": {"mimeType": "AUDIO_L16", "sampleRate": 24000}}}})
    );
    // Another voice and a fast rate: the voice name and a style.
    samples(&r.engine, "Again.", "Puck", 250);
    let body = r.server.requests()[1].json();
    assert_eq!(
        body["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
        "Puck"
    );
    assert_eq!(
        body["contents"][0]["parts"][0]["speechMetadata"]["style"],
        "speaking quickly"
    );
    assert_eq!(r.engine.chunk_chars(), 1000, "the reader joins sentences");
    assert_eq!(r.engine.lookahead(), 1, "a loopback url prefetches one");
}

#[test]
fn the_voice_list_is_the_prebuilt_voices_without_a_request() {
    let r = rig_with(gemini(), None, false, None);
    let voices = r.engine.refresh_voices().unwrap();
    assert_eq!(voices.len(), 30);
    assert!(voices.iter().any(|v| v.id == "Kore" && v.name == "Kore"));
    assert!(r.server.requests().is_empty());
    assert_eq!(r.engine.voices().len(), 30);
}

#[test]
fn a_refused_key_reads_with_the_fallback_and_the_cue_once() {
    let r = rig();
    r.server.on(
        SPEAK,
        rpc_error(
            400,
            "INVALID_ARGUMENT",
            "API key not valid. Please pass a valid API key.",
            json!([{"@type": "type.googleapis.com/google.rpc.ErrorInfo",
                "reason": "API_KEY_INVALID", "domain": "googleapis.com"}]),
        ),
    );
    assert_eq!(
        samples(&r.engine, "One.", "", 200),
        with_cue(Reason::Auth, "One.")
    );
    assert_eq!(
        samples(&r.engine, "Two.", "", 200),
        FakeEngine::render("Two.", "", 200).unwrap(),
        "the cue once"
    );
    assert_eq!(r.server.count(SPEAK), 1, "blocked: no second request");
    let st = r.engine.status();
    assert_eq!(st.readiness, Readiness::Unavailable);
    assert_eq!(st.reason, Some(Reason::Auth));
    assert!(!st.message.unwrap().contains(KEY));
}

#[test]
fn a_403_is_a_refused_key() {
    let r = rig_with(gemini(), Some(KEY), false, None);
    r.server.on(
        SPEAK,
        rpc_error(403, "PERMISSION_DENIED", "Permission denied.", json!([])),
    );
    match r.engine.test("Hi.", "", 200) {
        Err(Error::External { reason, message }) => {
            assert_eq!(reason, Reason::Auth);
            assert_eq!(message, "Gemini refused the key (403): Permission denied.");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unknown_voice_is_bad_voice_for_that_voice_only() {
    let r = rig();
    r.server.queue(
        SPEAK,
        rpc_error(
            400,
            "INVALID_ARGUMENT",
            "Voice name Nobody is not found.",
            json!([]),
        ),
    );
    r.server.on(SPEAK, audio(&[1, 2]));
    assert_eq!(
        samples(&r.engine, "One.", "Nobody", 200),
        with_cue(Reason::BadVoice, "One.")
    );
    assert_eq!(r.engine.status().reason, Some(Reason::BadVoice));
    // Another voice still reads with Gemini.
    assert_eq!(samples(&r.engine, "Two.", "Kore", 200), vec![1, 2]);
}

#[test]
fn a_429_waits_for_its_retry_delay_with_the_fallback_and_one_cue() {
    let r = rig();
    r.server.queue(
        SPEAK,
        rpc_error(
            429,
            "RESOURCE_EXHAUSTED",
            "You exceeded your current quota, please check your plan and billing details.",
            json!([
                {"@type": "type.googleapis.com/google.rpc.QuotaFailure",
                 "violations": [{"quotaId": "GenerateRequestsPerMinutePerProjectPerModel-FreeTier"}]},
                {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "39s"}]),
        ),
    );
    r.server.on(SPEAK, audio(&[7]));
    let start = std::time::Instant::now();
    assert_eq!(
        samples(&r.engine, "One.", "", 200),
        with_cue(Reason::RateLimited, "One.")
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "39 s is not waited for"
    );
    // The next sentences go straight to the fallback until the delay ends.
    assert_eq!(
        samples(&r.engine, "Two.", "", 200),
        FakeEngine::render("Two.", "", 200).unwrap()
    );
    assert_eq!(r.server.count(SPEAK), 1, "nothing sent before retryDelay");
    assert_eq!(r.engine.status().readiness, Readiness::Waiting);
    // A daily quota is out of credit.
    let r = rig();
    r.server.on(
        SPEAK,
        rpc_error(
            429,
            "RESOURCE_EXHAUSTED",
            "You exceeded your current quota.",
            json!([{"@type": "type.googleapis.com/google.rpc.QuotaFailure",
                "violations": [{"quotaId": "GenerateRequestsPerDayPerProjectPerModel-FreeTier"}]}]),
        ),
    );
    assert_eq!(
        samples(&r.engine, "One.", "", 200),
        with_cue(Reason::Quota, "One.")
    );
    assert_eq!(r.engine.status().reason, Some(Reason::Quota));
}

#[test]
fn a_model_that_refuses_response_format_is_asked_again_without_it() {
    let r = rig();
    r.server.queue(
        SPEAK,
        rpc_error(
            400,
            "INVALID_ARGUMENT",
            "Invalid JSON payload received. Unknown name \"responseFormat\" at \
             'generation_config': Cannot find field.",
            json!([]),
        ),
    );
    r.server.on(SPEAK, audio(&[3, 4]));
    assert_eq!(samples(&r.engine, "One.", "", 200), vec![3, 4]);
    let reqs = r.server.requests();
    assert_eq!(reqs.len(), 2, "once more, without the field");
    assert!(reqs[0].json()["generationConfig"]
        .get("responseFormat")
        .is_some());
    assert!(reqs[1].json()["generationConfig"]
        .get("responseFormat")
        .is_none());
    samples(&r.engine, "Two.", "", 200);
    assert!(
        r.server.requests()[2].json()["generationConfig"]
            .get("responseFormat")
            .is_none(),
        "remembered"
    );
}

#[test]
fn without_a_key_no_text_is_sent() {
    let r = rig_with(gemini(), None, true, None);
    r.server.on(SPEAK, audio(&[1]));
    assert_eq!(
        samples(&r.engine, "Secret.", "", 200),
        with_cue(Reason::NoKey, "Secret.")
    );
    assert!(r.server.requests().is_empty());
}

#[test]
fn muted_sends_nothing() {
    let hold = Arc::new(Hold::new());
    let r = rig_with(gemini(), Some(KEY), true, Some(hold.clone()));
    r.server.on(SPEAK, audio(&[1]));
    hold.set(true);
    assert_eq!(
        samples(&r.engine, "Quiet.", "", 200),
        FakeEngine::render("Quiet.", "", 200).unwrap()
    );
    let _ = r.engine.refresh_voices();
    assert!(r.server.requests().is_empty(), "nothing while muted");
    assert_eq!(r.fake.texts(), vec!["Quiet.".to_string()]);
    hold.set(false);
    assert_eq!(samples(&r.engine, "Loud.", "", 200), vec![1]);
    assert_eq!(r.server.count(SPEAK), 1);
}

#[test]
fn the_key_is_bound_to_googles_address() {
    // A key entered for Gemini's own address never goes to a url set later
    // (spec 6.4).
    let mut v = gemini();
    let home = Profile::from_json(&v).unwrap();
    assert_eq!(home.origin().as_deref(), Some(GOOGLE));
    let server = ScriptServer::start();
    server.on(SPEAK, audio(&[1]));
    v["url"] = json!(server.base);
    let store = Arc::new(MemoryStore::new());
    store.set("ge", &Secret::new(KEY), GOOGLE).unwrap();
    let mut config = ExternalConfig::new(Profile::from_json(&v).unwrap(), KeyResolver::new(store));
    config.fallback = Some(Arc::new(FakeEngine::new()) as Arc<dyn Engine>);
    let e = External::new(config).unwrap();
    assert!(!e.key_present());
    let _ = e
        .synthesize("Where does this go?", "", 200)
        .map(|i| i.count());
    let _ = e.test("Hello.", "", 200);
    assert!(!saw_a_key(&server), "the key left Google's address");
    assert!(server.requests().is_empty(), "no key: no text either");
}

#[test]
fn a_redirect_is_never_followed() {
    let r = rig();
    let other = ScriptServer::start();
    for status in [301, 302, 307] {
        r.server.queue(
            SPEAK,
            Route::new(status, "text/plain", b"moved".to_vec())
                .header("Location", &format!("{}{SPEAK}", other.base)),
        );
    }
    for _ in 0..3 {
        r.engine.reset();
        let _ = r.engine.synthesize("Hello.", "", 200).map(|i| i.count());
    }
    assert!(other.requests().is_empty(), "{:?}", other.requests());
    assert_eq!(r.server.count(SPEAK), 3);
}
