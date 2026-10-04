//! Kind `gemini` (#235) against a scripted local server standing in for
//! the Gemini API: the streamed request (path, key header, body), the
//! base64 PCM its events carry, playback from the first event, a first
//! audio too late (the fallback reads), the whole-answer path when the
//! stream is refused, the live model and voice lists, no model or voice in
//! code ("choose a model", "choose a voice"), failures that speak with the
//! fallback (a refused key, an unknown voice, a 429 with its retryDelay),
//! the key bound to Google's address, no redirects, and nothing sent while
//! muted. The real Gemini API is never called.
mod common;

use base64::Engine as _;
use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::error::cue_text;
use sonara_engine::external::hold::Hold;
use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig, Notice};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, Error, InputLimit, Readiness, Reason, SendMode};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const KEY: &str = "AIzaSyGeminiTestKey0123456789abcdefghi";
/// The streamed request (the default).
const SPEAK: &str = "/v1beta/models/tts-a:streamGenerateContent";
/// The whole answer (a model that refused the stream).
const WHOLE: &str = "/v1beta/models/tts-a:generateContent";
const GOOGLE: &str = "https://generativelanguage.googleapis.com:443";

/// A profile as the user sets it: a model and a voice picked from Google's
/// lists (none comes from Sonara).
fn gemini() -> Value {
    json!({"id": "ge", "kind": "gemini", "model": "tts-a", "voice": "Voice1",
        "options": {"timeout_ms": 5000}})
}

struct Rig {
    server: ScriptServer,
    engine: External,
    fake: Arc<FakeEngine>,
    notices: Arc<Mutex<Vec<Notice>>>,
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
    let notices: Arc<Mutex<Vec<Notice>>> = Arc::default();
    let n = notices.clone();
    config.notice = Some(Arc::new(move |x| n.lock().unwrap().push(x)));
    Rig {
        engine: External::new(config).unwrap(),
        server,
        fake,
        notices,
    }
}

fn rig() -> Rig {
    rig_with(gemini(), Some(KEY), true, None)
}

/// One `GenerateContentResponse` with these samples.
fn response(samples: &[i16], last: bool) -> String {
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut v = json!({"candidates": [{"content": {"role": "model", "parts": [{"inlineData":
        {"mimeType": "audio/L16;codec=pcm;rate=24000", "data": data}}]}}]});
    if last {
        v["candidates"][0]["finishReason"] = json!("STOP");
    }
    v.to_string()
}

/// The streamed answer: one event with all the audio.
fn audio(samples: &[i16]) -> Route {
    Route::sse(vec![(Duration::ZERO, response(samples, true))])
}

/// The whole answer of `generateContent`.
fn whole(samples: &[i16]) -> Route {
    Route::json(200, &response(samples, true))
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
    assert_eq!(
        req.path,
        format!("{SPEAK}?alt=sse"),
        "no key in the URL, only the stream format"
    );
    assert_eq!(req.header("x-goog-api-key"), Some(KEY));
    assert_eq!(req.header("authorization"), None);
    assert_eq!(
        req.json(),
        json!({"contents": [{"role": "user", "parts": [{"text": "Hello there."}]}],
            "generationConfig": {"responseModalities": ["AUDIO"],
                "speechConfig": {"voiceConfig": {"prebuiltVoiceConfig": {"voiceName": "Voice1"}}},
                "responseFormat": {"audio": {"mimeType": "AUDIO_L16", "sampleRate": 24000}}}})
    );
    // Another voice and a fast rate: the voice name and a style (the
    // default 250 wpm sends none).
    samples(&r.engine, "Again.", "Voice2", 320);
    let body = r.server.requests()[1].json();
    assert_eq!(
        body["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
        "Voice2"
    );
    assert_eq!(
        body["contents"][0]["parts"][0]["speechMetadata"]["style"],
        "speaking quickly"
    );
    // A whole message per request (#235), at most 2000 characters.
    assert_eq!(r.engine.send_mode(), SendMode::Message);
    assert_eq!(r.engine.input_limit(), InputLimit::Chars(2000));
    assert_eq!(r.engine.lookahead(), 1, "a loopback url prefetches one");
}

#[test]
fn voices_and_models_come_live_from_google_with_the_key() {
    // No list in code (#235): both are fetched with the key, paged.
    let r = rig();
    r.server.queue(
        "/v1beta/voices",
        Route::json(
            200,
            r#"{"voices": [{"id": "voice_mine", "display_name": "Mine", "type": "replicated"},
                {"id": "Voice1", "display_name": "Voice1", "language_code": "en-US"}],
                "next_page_token": "p2"}"#,
        ),
    );
    r.server.queue(
        "/v1beta/voices",
        Route::json(200, r#"{"voices": [{"id": "Voice3"}]}"#),
    );
    let voices = r.engine.refresh_voices().unwrap();
    let ids: Vec<&str> = voices.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, vec!["voice_mine", "Voice1", "Voice3"]);
    let reqs = r.server.requests();
    assert_eq!(reqs[0].path, "/v1beta/voices?page_size=1000");
    assert_eq!(reqs[1].path, "/v1beta/voices?page_size=1000&page_token=p2");
    assert!(reqs.iter().all(|q| q.header("x-goog-api-key") == Some(KEY)));
    r.server.on(
        "/v1beta/models",
        Route::json(
            200,
            r#"{"models": [{"name": "models/chat-1", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/tts-a", "displayName": "TTS A", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/tts-b", "supportedGenerationMethods": ["generateContent"]}]}"#,
        ),
    );
    assert!(r.engine.has_model_list());
    assert!(r.engine.models_stale());
    let models = r.engine.refresh_models().unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["tts-a", "tts-b"], "the speech models only");
    assert_eq!(models[0].name, "TTS A");
    assert!(!r.engine.models_stale(), "cached");
    // A refused key: the error, and no new fetch for a minute.
    let bad = rig();
    bad.server.on(
        "/v1beta/models",
        rpc_error(403, "PERMISSION_DENIED", "denied", json!([])),
    );
    match bad.engine.refresh_models() {
        Err(Error::External { reason, .. }) => assert_eq!(reason, Reason::Auth),
        other => panic!("{other:?}"),
    }
    assert!(!bad.engine.models_stale());
}

#[test]
fn audio_plays_from_the_first_event_while_the_rest_is_made() {
    // Review of #235: generateContent took 43 s for 2 s of audio. The
    // stream hands the first audio to the reader at once.
    let r = rig();
    r.server.on(
        SPEAK,
        Route::sse(vec![
            (Duration::ZERO, response(&[1, 2], false)),
            (Duration::from_millis(600), response(&[3], false)),
            (Duration::ZERO, response(&[4], true)),
        ]),
    );
    assert!(r.engine.streams(), "the reader plays it as it comes");
    let start = Instant::now();
    let mut stream = r.engine.synthesize("A longer text.", "", 200).unwrap();
    let first = stream.next().unwrap().unwrap();
    assert_eq!(first.samples, vec![1, 2]);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "the first audio before the rest: {:?}",
        start.elapsed()
    );
    let rest: Vec<i16> = stream.flat_map(|c| c.unwrap().samples).collect();
    assert_eq!(rest, vec![3, 4]);
    assert_eq!(r.engine.status().readiness, Readiness::Ready);
    assert_eq!(r.fake.syntheses(), 0);
}

#[test]
fn a_first_audio_too_late_reads_with_the_fallback_and_the_cue() {
    let mut v = gemini();
    v["options"]["first_audio_ms"] = json!(1000);
    let r = rig_with(v, Some(KEY), true, None);
    r.server.on(
        SPEAK,
        Route::sse(vec![(Duration::from_secs(4), response(&[9], true))]),
    );
    let start = Instant::now();
    assert_eq!(
        samples(&r.engine, "Slow.", "", 200),
        with_cue(Reason::Timeout, "Slow.")
    );
    assert!(
        start.elapsed() < Duration::from_millis(2500),
        "the first-audio limit, not the whole timeout: {:?}",
        start.elapsed()
    );
    let st = r.engine.status();
    assert_eq!(st.reason, None, "one timeout alone opens no breaker");
    // A header that comes late is the same.
    let r = rig_with(
        {
            let mut v = gemini();
            v["options"]["first_audio_ms"] = json!(1000);
            v
        },
        Some(KEY),
        true,
        None,
    );
    r.server
        .on(SPEAK, audio(&[9]).delayed(Duration::from_secs(4)));
    let start = Instant::now();
    assert_eq!(
        samples(&r.engine, "Late.", "", 200),
        with_cue(Reason::Timeout, "Late.")
    );
    assert!(start.elapsed() < Duration::from_millis(2500));
}

#[test]
fn a_stream_that_stops_keeps_the_audio_it_sent() {
    // Past the whole timeout after the first audio: the chunk ends with
    // what came, and the log says why (no fallback repeats spoken text).
    let mut v = gemini();
    v["options"]["timeout_ms"] = json!(1500);
    v["options"]["first_audio_ms"] = json!(1000);
    let r = rig_with(v, Some(KEY), true, None);
    r.server.on(
        SPEAK,
        Route::sse(vec![
            (Duration::ZERO, response(&[1], false)),
            (Duration::from_secs(5), response(&[2], true)),
        ]),
    );
    let got = samples(&r.engine, "Cut.", "", 200);
    assert_eq!(got, vec![1]);
    assert_eq!(r.fake.syntheses(), 0, "no fallback repeats spoken text");
    let n = r.notices.lock().unwrap().clone();
    assert!(
        n.iter()
            .any(|x| x.reason == Some(Reason::Timeout) && x.message.contains("did not finish")),
        "{n:?}"
    );
}

#[test]
fn a_sample_split_between_events_plays_whole() {
    // Review of #235: Google may cut the PCM in the middle of a sample
    // between events; the half sample waits for the next event.
    let r = rig();
    let bytes: Vec<u8> = [5i16, -6, 7].iter().flat_map(|s| s.to_le_bytes()).collect();
    let event = |b: &[u8]| {
        json!({"candidates": [{"content": {"role": "model", "parts": [{"inlineData":
            {"mimeType": "audio/L16;codec=pcm;rate=24000",
            "data": base64::engine::general_purpose::STANDARD.encode(b)}}]}}]})
        .to_string()
    };
    r.server.on(
        SPEAK,
        Route::sse(vec![
            (Duration::ZERO, event(&bytes[..3])),
            (Duration::ZERO, event(&bytes[3..])),
        ]),
    );
    assert_eq!(samples(&r.engine, "Split.", "", 200), vec![5, -6, 7]);
    assert_eq!(r.fake.syntheses(), 0);
}

#[test]
fn a_refused_stream_reads_whole_answers_from_then_on() {
    let r = rig();
    r.server.on(
        SPEAK,
        rpc_error(
            404,
            "NOT_FOUND",
            "models/tts-a is not found for API version v1beta, or is not supported for streamGenerateContent.",
            json!([]),
        ),
    );
    r.server.on(WHOLE, whole(&[6, 7]));
    assert_eq!(samples(&r.engine, "One.", "", 200), vec![6, 7]);
    // Still played as it comes (a whole message, send mode `message`).
    assert!(r.engine.streams());
    assert_eq!(samples(&r.engine, "Two.", "", 200), vec![6, 7]);
    assert_eq!(r.server.count(SPEAK), 1, "the stream is asked for once");
    assert_eq!(r.server.count(WHOLE), 2);
    assert_eq!(r.engine.status().readiness, Readiness::Ready);
}

#[test]
fn a_retired_model_says_so_and_names_it() {
    let r = rig();
    r.server.on(
        SPEAK,
        rpc_error(
            404,
            "NOT_FOUND",
            "models/tts-a is not found for API version v1beta, or is not supported for streamGenerateContent.",
            json!([]),
        ),
    );
    r.server.on(
        WHOLE,
        rpc_error(
            404,
            "NOT_FOUND",
            "models/tts-a is not found for API version v1beta, or is not supported for generateContent.",
            json!([]),
        ),
    );
    assert_eq!(
        samples(&r.engine, "One.", "", 200),
        with_cue(Reason::BadConfig, "One.")
    );
    let st = r.engine.status();
    assert_eq!(st.reason, Some(Reason::BadConfig));
    let m = st.message.unwrap();
    assert!(
        m.starts_with("Gemini does not know the model 'tts-a' (404)"),
        "{m}"
    );
}

#[test]
fn no_model_and_no_voice_in_code_choose_them() {
    // A profile stored before #235 relied on a default; now it says what
    // to pick, reads with the fallback, and sends nothing.
    let r = rig_with(json!({"id": "ge", "kind": "gemini"}), Some(KEY), true, None);
    let st = r.engine.status();
    assert_eq!(
        (st.readiness, st.reason),
        (Readiness::Unavailable, Some(Reason::BadConfig))
    );
    assert_eq!(
        st.message.as_deref(),
        Some("Choose a model for Gemini in Sonara's settings (Engines).")
    );
    assert_eq!(
        samples(&r.engine, "Hello.", "", 200),
        with_cue(Reason::BadConfig, "Hello.")
    );
    assert!(r.server.requests().is_empty(), "nothing sent");
    match r.engine.test("Hi.", "Voice1", 200) {
        Err(Error::External { reason, message }) => {
            assert_eq!(reason, Reason::BadConfig);
            assert!(message.starts_with("Choose a model"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    // A model but no voice: "choose a voice" until the reader names one.
    let r = rig_with(
        json!({"id": "ge", "kind": "gemini", "model": "tts-a"}),
        Some(KEY),
        true,
        None,
    );
    r.server.on(SPEAK, audio(&[4]));
    assert_eq!(
        samples(&r.engine, "Hello.", "", 200),
        with_cue(Reason::BadConfig, "Hello.")
    );
    assert_eq!(
        r.engine.status().message.as_deref(),
        Some("Choose a voice for Gemini in Sonara's settings.")
    );
    assert!(r.server.requests().is_empty());
    // The user's voice setting names one: it reads, and the status clears.
    assert_eq!(samples(&r.engine, "Hello.", "Voice2", 200), vec![4]);
    assert_eq!(r.engine.status().readiness, Readiness::Ready);
    let body = r.server.requests()[0].json();
    assert_eq!(
        body["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
        "Voice2"
    );
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
    assert_eq!(samples(&r.engine, "Two.", "Voice1", 200), vec![1, 2]);
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

#[test]
fn a_model_that_refuses_both_fields_loses_each_once_and_still_reads() {
    // Review of #235: a model that knows neither `responseFormat` nor
    // `speechMetadata` costs one request per field, then reads; the
    // engine is never blocked as "settings do not work".
    let r = rig();
    let refuse = |field: &str| {
        rpc_error(
            400,
            "INVALID_ARGUMENT",
            &format!("Invalid JSON payload received. Unknown name \"{field}\": Cannot find field."),
            json!([]),
        )
    };
    r.server.queue(SPEAK, refuse("responseFormat"));
    r.server.queue(SPEAK, refuse("speechMetadata"));
    r.server.on(SPEAK, audio(&[3, 4]));
    // A slow rate carries a style, so both fields are in the first request.
    assert_eq!(samples(&r.engine, "One.", "", 100), vec![3, 4]);
    let reqs = r.server.requests();
    assert_eq!(reqs.len(), 3, "one more request per refused field");
    let third = reqs[2].json();
    assert!(third["generationConfig"].get("responseFormat").is_none());
    assert!(third["contents"][0]["parts"][0]
        .get("speechMetadata")
        .is_none());
    assert_eq!(r.engine.status().reason, None, "not blocked");
    assert_eq!(samples(&r.engine, "Two.", "", 100), vec![3, 4]);
    assert_eq!(r.server.count(SPEAK), 4, "remembered: one request");
}

#[test]
fn a_daily_quota_is_not_retried_at_once_and_waits_past_the_reset() {
    // Review of #235: a per-day 429 with a short retryDelay is neither
    // retried at once nor probed every ten minutes.
    let r = rig();
    r.server.on(
        SPEAK,
        rpc_error(
            429,
            "RESOURCE_EXHAUSTED",
            "You exceeded your current quota.",
            json!([
                {"@type": "type.googleapis.com/google.rpc.QuotaFailure",
                 "violations": [{"quotaId": "GenerateRequestsPerDayPerProjectPerModel-FreeTier"}]},
                {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "1s"}]),
        ),
    );
    assert_eq!(
        samples(&r.engine, "One.", "", 200),
        with_cue(Reason::Quota, "One.")
    );
    assert_eq!(r.server.count(SPEAK), 1, "a quota is not retried at once");
    assert_eq!(r.engine.status().reason, Some(Reason::Quota));
}
