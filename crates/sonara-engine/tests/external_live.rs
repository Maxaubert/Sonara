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
//! - `elevenlabs_live`: `ELEVENLABS_API_KEY`, optional
//!   `SONARA_LIVE_ELEVENLABS_VOICE` (default: the premade voice George)
//! - `azure_live`: `AZURE_SPEECH_KEY`, `AZURE_SPEECH_REGION`
//! - `google_live`: `GOOGLE_TTS_API_KEY` (also the acceptance check of API-key
//!   auth through `X-goog-api-key`, spec 13.3 item 1)
//! - `gemini_live`: `GEMINI_API_KEY`, optional `SONARA_LIVE_GEMINI_MODEL`
//!   (default `gemini-3.8-flash-lite-tts`); one request for the sentence,
//!   none for the voices (a fixed list)
//! - `cartesia_live`: `CARTESIA_API_KEY`, `SONARA_LIVE_CARTESIA_VOICE`
//! - `deepgram_live`: `DEEPGRAM_API_KEY` (also prints whether Deepgram
//!   applies `speed`, spec 13.3 item 2)
//! - `command_live`: `SONARA_LIVE_COMMAND` (a JSON argv of a real program of
//!   your own), optional `SONARA_LIVE_COMMAND_OPTIONS` (a JSON object)
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
    println!("{} voices", voices.len());
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

#[test]
#[ignore]
fn elevenlabs_live() {
    if var("ELEVENLABS_API_KEY").is_none() {
        println!("skipped: set ELEVENLABS_API_KEY");
        return;
    }
    let voice =
        var("SONARA_LIVE_ELEVENLABS_VOICE").unwrap_or_else(|| "JBFqnCBsd6RMkjVDRZzb".into());
    check(
        json!({"id": "elevenlabs-live", "kind": "elevenlabs", "voice": voice,
        "key_ref": "env:ELEVENLABS_API_KEY"}),
    );
}

#[test]
#[ignore]
fn azure_live() {
    let (Some(_), Some(region)) = (var("AZURE_SPEECH_KEY"), var("AZURE_SPEECH_REGION")) else {
        println!("skipped: set AZURE_SPEECH_KEY and AZURE_SPEECH_REGION");
        return;
    };
    check(
        json!({"id": "azure-live", "kind": "azure", "voice": "en-US-AvaMultilingualNeural",
        "key_ref": "env:AZURE_SPEECH_KEY", "options": {"region": region}}),
    );
}

#[test]
#[ignore]
fn google_live() {
    if var("GOOGLE_TTS_API_KEY").is_none() {
        println!("skipped: set GOOGLE_TTS_API_KEY");
        return;
    }
    check(
        json!({"id": "google-live", "kind": "google", "voice": "en-US-Chirp3-HD-Kore",
        "key_ref": "env:GOOGLE_TTS_API_KEY"}),
    );
}

#[test]
#[ignore]
fn gemini_live() {
    if var("GEMINI_API_KEY").is_none() {
        println!("skipped: set GEMINI_API_KEY");
        return;
    }
    let mut p = json!({"id": "gemini-live", "kind": "gemini", "voice": "Kore",
        "key_ref": "env:GEMINI_API_KEY"});
    if let Some(m) = var("SONARA_LIVE_GEMINI_MODEL") {
        p["model"] = json!(m);
    }
    check(p);
}

#[test]
#[ignore]
fn cartesia_live() {
    let (Some(_), Some(voice)) = (var("CARTESIA_API_KEY"), var("SONARA_LIVE_CARTESIA_VOICE"))
    else {
        println!("skipped: set CARTESIA_API_KEY and SONARA_LIVE_CARTESIA_VOICE (a voice id)");
        return;
    };
    check(
        json!({"id": "cartesia-live", "kind": "cartesia", "voice": voice,
        "key_ref": "env:CARTESIA_API_KEY"}),
    );
}

/// Also spec 13.3 fact 2: whether Deepgram takes `speed`. A refused speed
/// is dropped (the synthesis still works), so the two durations say it: at
/// 250 wpm the audio is shorter than at 200 only when speed was applied.
#[test]
#[ignore]
fn deepgram_live() {
    if var("DEEPGRAM_API_KEY").is_none() {
        println!("skipped: set DEEPGRAM_API_KEY");
        return;
    }
    let profile = json!({"id": "deepgram-live", "kind": "deepgram",
        "voice": "aura-2-thalia-en", "key_ref": "env:DEEPGRAM_API_KEY"});
    check(profile.clone());
    let e = External::new(ExternalConfig::new(
        Profile::from_json(&profile).unwrap(),
        KeyResolver::new(Arc::new(MemoryStore::new())),
    ))
    .unwrap();
    let len = |wpm| -> usize {
        e.test("One two three four five six seven eight.", "", wpm)
            .expect("one synthesis")
            .pcm
            .iter()
            .map(|c| c.samples.len())
            .sum()
    };
    let (normal, fast) = (len(200), len(250));
    println!(
        "speed: {normal} samples at 200 wpm, {fast} at 250 wpm: {}",
        if fast * 10 < normal * 9 {
            "Deepgram applies speed"
        } else {
            "Deepgram ignored or refused speed"
        }
    );
}

/// A real local program of the user's own (for example Piper):
/// `SONARA_LIVE_COMMAND` is its argv as JSON, `SONARA_LIVE_COMMAND_OPTIONS`
/// optional further options (`input`, `output`, `sample_rate`, `voices`).
#[test]
#[ignore]
fn command_live() {
    let Some(argv) = var("SONARA_LIVE_COMMAND") else {
        println!("skipped: set SONARA_LIVE_COMMAND (a JSON argv such as [\"C:\\piper\\piper.exe\", ...])");
        return;
    };
    let argv: Value = serde_json::from_str(&argv).expect("SONARA_LIVE_COMMAND is a JSON array");
    let mut options = json!({"argv": argv});
    if let Some(more) = var("SONARA_LIVE_COMMAND_OPTIONS") {
        let more: Value = serde_json::from_str(&more).expect("a JSON object");
        for (k, v) in more.as_object().expect("a JSON object") {
            options[k] = v.clone();
        }
    }
    check(json!({"id": "command-live", "kind": "command", "options": options}));
}
