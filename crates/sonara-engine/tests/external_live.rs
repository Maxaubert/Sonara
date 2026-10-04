//! Live checks of external engines against real providers and servers
//! (opt-in, never in CI): `cargo test -p sonara-engine --test external_live
//! -- --ignored --nocapture`. Keys and addresses come from environment
//! variables; a test whose variables are unset says so and passes.
//!
//! - `openai_live`: `OPENAI_API_KEY`
//! - `kokoro_fastapi_live`: `SONARA_LIVE_KOKORO_FASTAPI_URL` (e.g.
//!   `http://127.0.0.1:8880/v1`)
//! - `localai_live`: `SONARA_LIVE_LOCALAI_URL`, `SONARA_LIVE_LOCALAI_MODEL`
//! - `speaches_live`: `SONARA_LIVE_SPEACHES_URL`, `SONARA_LIVE_SPEACHES_MODEL`
use serde_json::{json, Value};
use sonara_engine::external::keys::{KeyResolver, MemoryStore};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig};
use sonara_engine::Engine;
use std::sync::Arc;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn check(profile: Value) {
    let p = Profile::from_json(&profile).expect("a valid profile");
    let e = External::new(ExternalConfig::new(
        p,
        KeyResolver::new(Arc::new(MemoryStore::new())),
    ))
    .unwrap();
    let voices = e.refresh_voices().expect("the voice list");
    println!("{} voices: {}", voices.len(), voices.len());
    let t = e
        .test("Hello. This is a live check of Sonara.", "", 250)
        .expect("one synthesis");
    let samples: usize = t.pcm.iter().map(|c| c.samples.len()).sum();
    let rate = t.pcm[0].sample_rate;
    println!(
        "voice {} in {} ms: {samples} samples at {rate} Hz",
        t.voice, t.ms
    );
    assert!(samples as u32 > rate / 2, "at least half a second of audio");
    assert!(
        t.pcm[0].samples.iter().any(|s| s.unsigned_abs() > 500),
        "not silence"
    );
}

#[test]
#[ignore]
fn openai_live() {
    if var("OPENAI_API_KEY").is_none() {
        println!("skipped: set OPENAI_API_KEY");
        return;
    }
    check(json!({"id": "openai-live", "kind": "openai-compatible",
        "key_ref": "env:OPENAI_API_KEY", "options": {"preset": "openai"}}));
}

#[test]
#[ignore]
fn kokoro_fastapi_live() {
    let Some(url) = var("SONARA_LIVE_KOKORO_FASTAPI_URL") else {
        println!("skipped: set SONARA_LIVE_KOKORO_FASTAPI_URL");
        return;
    };
    check(
        json!({"id": "kfa-live", "kind": "openai-compatible", "url": url,
        "options": {"preset": "kokoro-fastapi"}}),
    );
}

#[test]
#[ignore]
fn localai_live() {
    let (Some(url), Some(model)) = (
        var("SONARA_LIVE_LOCALAI_URL"),
        var("SONARA_LIVE_LOCALAI_MODEL"),
    ) else {
        println!("skipped: set SONARA_LIVE_LOCALAI_URL and SONARA_LIVE_LOCALAI_MODEL");
        return;
    };
    check(
        json!({"id": "localai-live", "kind": "openai-compatible", "url": url,
        "model": model, "key_ref": "none", "options": {"preset": "localai"}}),
    );
}

#[test]
#[ignore]
fn speaches_live() {
    let (Some(url), Some(model)) = (
        var("SONARA_LIVE_SPEACHES_URL"),
        var("SONARA_LIVE_SPEACHES_MODEL"),
    ) else {
        println!("skipped: set SONARA_LIVE_SPEACHES_URL and SONARA_LIVE_SPEACHES_MODEL");
        return;
    };
    check(
        json!({"id": "speaches-live", "kind": "openai-compatible", "url": url,
        "model": model, "key_ref": "none", "options": {"preset": "speaches"}}),
    );
}
