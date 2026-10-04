//! The `External` engine's core behaviour (spec 7, 8) against a scripted
//! local server: fallback with the cue, the breaker, `Retry-After`, cancel,
//! split chunks, the cue cache and keys. No real provider is called.
mod common;

use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::error::cue_text;
use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig, Notice};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, EngineId, Error, Readiness, Reason};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SPEECH: &str = "/v1/audio/speech";

struct Rig {
    server: ScriptServer,
    fake: Arc<FakeEngine>,
    engine: Arc<External>,
    store: Arc<MemoryStore>,
    notices: Arc<Mutex<Vec<Notice>>>,
}

fn profile(base: &str, extra: Value) -> Profile {
    let mut v = json!({"id": "core-test", "kind": "openai-compatible", "label": "Test",
        "voice": "v1", "url": format!("{base}/v1"), "options": {"preset": "generic", "timeout_ms": 5000}});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    Profile::from_json(&v).unwrap()
}

fn rig_with(extra: Value, fallback: bool) -> Rig {
    let server = ScriptServer::start();
    let fake = Arc::new(FakeEngine::new());
    let store = Arc::new(MemoryStore::new());
    let notices = Arc::new(Mutex::new(Vec::new()));
    let mut config = ExternalConfig::new(
        profile(&server.base, extra),
        KeyResolver::new(store.clone()),
    );
    if fallback {
        config.fallback = Some(fake.clone() as Arc<dyn Engine>);
    }
    let n = notices.clone();
    config.notice = Some(Arc::new(move |x| n.lock().unwrap().push(x)));
    let engine = Arc::new(External::new(config).unwrap());
    Rig {
        server,
        fake,
        engine,
        store,
        notices,
    }
}

fn rig() -> Rig {
    rig_with(json!({}), true)
}

fn samples(engine: &External, text: &str) -> Result<Vec<i16>, Error> {
    Ok(engine
        .synthesize(text, "", 200)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flat_map(|c| c.samples)
        .collect())
}

fn fake_audio(text: &str) -> Vec<i16> {
    FakeEngine::render(text, "", 200).unwrap()
}

#[test]
fn a_working_provider_speaks_its_own_audio() {
    let r = rig();
    r.server.on(SPEECH, Route::wav(&[1, 2, 3, 4], 24_000));
    assert_eq!(samples(&r.engine, "Hello.").unwrap(), vec![1, 2, 3, 4]);
    assert_eq!(r.fake.syntheses(), 0);
    assert_eq!(r.engine.status().readiness, Readiness::Ready);
}

#[test]
fn failure_speaks_the_chunk_with_the_fallback_and_prepends_the_cue_once() {
    let r = rig();
    r.server.on(
        SPEECH,
        Route::json(500, r#"{"error": {"message": "boom"}}"#),
    );
    let cue = cue_text(Reason::Server, "Test");
    assert_eq!(
        cue,
        "Test has a server problem. Reading with the built-in voice."
    );
    let mut want = fake_audio(&cue);
    want.extend(fake_audio("Hello."));
    assert_eq!(samples(&r.engine, "Hello.").unwrap(), want);
    // The next failure in the same episode has no cue.
    assert_eq!(samples(&r.engine, "Again.").unwrap(), fake_audio("Again."));
    assert_eq!(r.fake.texts(), vec![cue, "Hello.".into(), "Again.".into()]);
    let n = r.notices.lock().unwrap().clone();
    assert_eq!(n[0].reason, Some(Reason::Server));
    assert_eq!(n[0].status, Some(500));
    assert_eq!(n[0].fallback, Some(EngineId("fake")));
    assert!(n[0].message.contains("boom"));
}

#[test]
fn breaker_open_skips_the_network() {
    let r = rig();
    r.server.on(SPEECH, Route::json(502, ""));
    samples(&r.engine, "One.").unwrap();
    samples(&r.engine, "Two.").unwrap();
    assert_eq!(r.server.count(SPEECH), 2);
    let start = Instant::now();
    samples(&r.engine, "Three.").unwrap();
    assert_eq!(
        r.server.count(SPEECH),
        2,
        "no request while the breaker is open"
    );
    assert!(start.elapsed() < Duration::from_millis(200));
    let s = r.engine.status();
    assert_eq!(s.readiness, Readiness::Waiting);
    assert_eq!(s.reason, Some(Reason::Server));
    assert_eq!(s.fallback, Some(EngineId("fake")));
}

#[test]
fn retry_after_once_then_fallback() {
    let r = rig();
    let busy = || {
        Route::json(
            429,
            r#"{"error": {"code": "slow_down", "message": "slow down"}}"#,
        )
        .header("Retry-After", "0.1")
    };
    r.server.queue(SPEECH, busy());
    r.server.queue(SPEECH, busy());
    r.server.on(SPEECH, Route::wav(&[9], 24_000));
    let out = samples(&r.engine, "Hi.").unwrap();
    assert_eq!(r.server.count(SPEECH), 2, "one retry, never more");
    assert!(out.ends_with(&fake_audio("Hi.")), "the fallback spoke it");
    // A retry that succeeds is the provider's audio.
    r.server.queue(SPEECH, busy());
    assert_eq!(samples(&r.engine, "Ok.").unwrap(), vec![9]);
    assert_eq!(r.server.count(SPEECH), 4);
    // A long Retry-After is not waited for.
    r.server.queue(
        SPEECH,
        Route::json(429, r#"{"error": {"code": "slow_down"}}"#).header("Retry-After", "30"),
    );
    let start = Instant::now();
    samples(&r.engine, "Later.").unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn no_fallback_is_an_external_error() {
    let r = rig_with(json!({}), false);
    r.server.on(
        SPEECH,
        Route::json(401, r#"{"error": {"message": "bad key"}}"#),
    );
    match samples(&r.engine, "Hi.") {
        Err(Error::External { reason, message }) => {
            assert_eq!(reason, Reason::Auth);
            assert!(
                message.contains("refused the key (401): bad key"),
                "{message}"
            );
        }
        other => panic!("expected External, got {other:?}"),
    }
}

#[test]
fn split_parts_failure_uses_fallback_for_the_whole_chunk() {
    let r = rig();
    let long = "word ".repeat(1000);
    let long = long.trim();
    r.server.queue(SPEECH, Route::wav(&[1], 24_000));
    r.server.queue(SPEECH, Route::json(500, ""));
    let out = samples(&r.engine, long).unwrap();
    assert_eq!(r.server.count(SPEECH), 2, "the parts go in order");
    assert!(out.ends_with(&fake_audio(long)), "the whole chunk, once");
    assert_eq!(r.fake.texts().last().map(String::as_str), Some(long));
    // Both parts of a working provider make one synthesis.
    r.server.queue(SPEECH, Route::wav(&[1], 24_000));
    r.server.queue(SPEECH, Route::wav(&[2], 24_000));
    r.engine.reset();
    assert_eq!(samples(&r.engine, long).unwrap(), vec![1, 2]);
}

#[test]
fn cancel_returns_promptly_while_the_server_delays() {
    let r = rig();
    r.server.on(
        SPEECH,
        Route::wav(&[1], 24_000).delayed(Duration::from_secs(3)),
    );
    let engine = r.engine.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        let at = Instant::now();
        engine.cancel();
        at
    });
    let result = r.engine.synthesize("Slow.", "", 200);
    let ended = Instant::now();
    let cancelled_at = canceller.join().unwrap();
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(
        ended.duration_since(cancelled_at) < Duration::from_millis(200),
        "ended {:?} after the cancel",
        ended.duration_since(cancelled_at)
    );
    assert_eq!(r.fake.syntheses(), 0, "a cancel is not a failure");
}

#[test]
fn short_texts_are_served_from_the_cue_cache() {
    let r = rig();
    r.server.on(SPEECH, Route::wav(&[5, 6], 24_000));
    assert_eq!(samples(&r.engine, "Paused.").unwrap(), vec![5, 6]);
    assert_eq!(samples(&r.engine, "Paused.").unwrap(), vec![5, 6]);
    assert_eq!(r.server.count(SPEECH), 1);
    // Another rate is another entry; fallback audio is never cached.
    r.engine
        .synthesize("Paused.", "", 300)
        .unwrap()
        .for_each(drop);
    assert_eq!(r.server.count(SPEECH), 2);
    r.server.on(SPEECH, Route::json(500, ""));
    samples(&r.engine, "Down.").unwrap();
    r.server.on(SPEECH, Route::wav(&[7], 24_000));
    r.engine.reset();
    assert_eq!(samples(&r.engine, "Down.").unwrap(), vec![7]);
}

#[test]
fn a_missing_key_falls_back_without_a_request_until_one_is_set() {
    let r = rig_with(json!({"key_ref": "credman"}), true);
    r.server.on(SPEECH, Route::wav(&[3], 24_000));
    r.engine.warm().unwrap();
    let s = r.engine.status();
    assert_eq!(s.readiness, Readiness::Unavailable);
    assert_eq!(s.reason, Some(Reason::NoKey));
    let out = samples(&r.engine, "Hi.").unwrap();
    assert_eq!(r.server.count(SPEECH), 0);
    let mut want = fake_audio(&cue_text(Reason::NoKey, "Test"));
    want.extend(fake_audio("Hi."));
    assert_eq!(out, want);
    // The episode's first fallback is logged once with its reason.
    samples(&r.engine, "Again.").unwrap();
    let n = r.notices.lock().unwrap().clone();
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].reason, Some(Reason::NoKey));
    r.store
        .set(
            "core-test",
            &Secret::new("sk-local"),
            &r.engine.profile().origin().unwrap(),
        )
        .unwrap();
    assert!(r.engine.key_present());
    assert_eq!(samples(&r.engine, "Now.").unwrap(), vec![3]);
    let seen = r.server.requests();
    assert_eq!(seen[0].header("authorization"), Some("Bearer sk-local"));
    assert_eq!(r.engine.status().readiness, Readiness::Ready);
}

#[test]
fn an_auth_failure_blocks_until_a_new_key() {
    let r = rig_with(json!({"key_ref": "credman"}), true);
    r.store
        .set(
            "core-test",
            &Secret::new("sk-old"),
            &r.engine.profile().origin().unwrap(),
        )
        .unwrap();
    r.server.on(
        SPEECH,
        Route::json(401, r#"{"error": {"message": "nope"}}"#),
    );
    samples(&r.engine, "One.").unwrap();
    samples(&r.engine, "Two.").unwrap();
    assert_eq!(r.server.count(SPEECH), 1, "blocked after the first refusal");
    assert_eq!(r.engine.status().reason, Some(Reason::Auth));
    r.server.on(SPEECH, Route::wav(&[4], 24_000));
    r.store
        .set(
            "core-test",
            &Secret::new("sk-new"),
            &r.engine.profile().origin().unwrap(),
        )
        .unwrap();
    r.engine.key_changed();
    assert_eq!(samples(&r.engine, "Three.").unwrap(), vec![4]);
    let n = r.notices.lock().unwrap().clone();
    assert_eq!(n.last().unwrap().reason, None, "a recovery notice");
}

#[test]
fn test_has_no_fallback_and_clears_the_block() {
    let r = rig();
    r.server.on(SPEECH, Route::json(503, ""));
    samples(&r.engine, "A.").unwrap();
    samples(&r.engine, "B.").unwrap();
    assert_eq!(r.engine.status().readiness, Readiness::Waiting);
    let fallbacks = r.fake.syntheses();
    assert!(matches!(
        r.engine.test("Hello.", "", 200),
        Err(Error::External {
            reason: Reason::Server,
            ..
        })
    ));
    assert_eq!(r.fake.syntheses(), fallbacks, "no fallback in a test");
    r.server.on(SPEECH, Route::wav(&[1; 2400], 24_000));
    let t = r.engine.test("Hello.", "nova", 200).unwrap();
    assert_eq!(t.voice, "nova");
    assert_eq!(t.pcm[0].samples.len(), 2400);
    assert_eq!(r.engine.status().readiness, Readiness::Ready);
}

#[test]
fn voices_list_the_profile_voice_and_accept_any_id() {
    let r = rig_with(json!({"voice": "my-clone"}), true);
    assert!(r.engine.accepts_unlisted_voices());
    assert_eq!(r.engine.lookahead(), 1, "a loopback server: one ahead");
    let v = r.engine.voices();
    assert_eq!(v[0].id, "my-clone");
    assert_eq!(v[0].engine, EngineId::intern("core-test"));
    assert_eq!(v[0].license_class, sonara_engine::LicenseClass::External);
}

#[test]
fn a_cancel_after_begin_ends_the_synthesis_before_any_request() {
    // The reader calls `begin` under its queue lock and `synthesize` after
    // releasing it; a cancel in between must still end that synthesis.
    let r = rig_with(json!({}), true);
    r.server.on(SPEECH, Route::wav(&[1], 24_000));
    r.engine.begin();
    r.engine.cancel();
    assert!(matches!(
        r.engine.synthesize("Stale.", "", 200).map(|s| s.count()),
        Err(Error::Cancelled)
    ));
    assert_eq!(r.server.count(SPEECH), 0, "no request for a stale chunk");
    // The next synthesis is not affected.
    assert_eq!(samples(&r.engine, "Fresh.").unwrap(), vec![1]);
}

#[test]
fn a_redirect_is_never_followed_to_another_host() {
    // A provider's 3xx would carry custom key headers (xi-api-key and the
    // like) to the host in Location: the request ends there and falls back
    // (review of #224).
    let r = rig_with(json!({"key_ref": "credman"}), true);
    r.store
        .set(
            "core-test",
            &Secret::new("sk-bound"),
            &r.engine.profile().origin().unwrap(),
        )
        .unwrap();
    let other = ScriptServer::start();
    other.on(SPEECH, Route::wav(&[9, 9], 24_000));
    for status in [301, 302, 303, 307, 308] {
        r.server.queue(
            SPEECH,
            Route::new(status, "text/plain", b"moved".to_vec())
                .header("Location", &format!("{}{SPEECH}", other.base)),
        );
    }
    for _ in 0..5 {
        r.engine.reset();
        let out = samples(&r.engine, "Hi.").unwrap();
        assert_ne!(out, vec![9, 9]);
    }
    assert!(other.requests().is_empty(), "{:?}", other.requests());
    assert_eq!(r.server.count(SPEECH), 5);
}
