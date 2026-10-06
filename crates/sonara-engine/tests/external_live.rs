//! Live checks of external engines against real providers and servers
//! (opt-in, never in CI): `cargo test -p sonara-engine --test external_live
//! -- --ignored --nocapture`. Keys and addresses come from environment
//! variables; a test whose variables are unset says so and passes.
//!
//! No model or voice is named here (#235): models and voices change
//! upstream. Each check takes `SONARA_LIVE_<NAME>_MODEL` and
//! `SONARA_LIVE_<NAME>_VOICE` when set, else the first model and voice the
//! provider lists live; a provider without a voice list (OpenAI) needs the
//! voice variable.
//!
//! - `openai_live`: `OPENAI_API_KEY`, `SONARA_LIVE_OPENAI_VOICE`
//! - `kokoro_fastapi_live`: `SONARA_LIVE_KOKORO_FASTAPI_URL` (e.g.
//!   `http://127.0.0.1:8880/v1`)
//! - `localai_live`: `SONARA_LIVE_LOCALAI_URL`, `SONARA_LIVE_LOCALAI_MODEL`
//! - `speaches_live`: `SONARA_LIVE_SPEACHES_URL`, `SONARA_LIVE_SPEACHES_MODEL`
//! - `elevenlabs_live`: `ELEVENLABS_API_KEY`
//! - `azure_live`: `AZURE_SPEECH_KEY`, `AZURE_SPEECH_REGION`
//! - `google_live`: `GOOGLE_TTS_API_KEY` (also the acceptance check of API-key
//!   auth through `X-goog-api-key`, spec 13.3 item 1)
//! - `gemini_live`: `GEMINI_API_KEY`; the model and voice come from
//!   Google's lists, the sentence is streamed
//! - `cartesia_live`: `CARTESIA_API_KEY`, `SONARA_LIVE_CARTESIA_MODEL`
//! - `deepgram_live`: `DEEPGRAM_API_KEY` (also prints whether Deepgram
//!   applies `speed`, spec 13.3 item 2)
//! - `command_live`: `SONARA_LIVE_COMMAND` (a JSON argv of a real program of
//!   your own), optional `SONARA_LIVE_COMMAND_OPTIONS` (a JSON object) and,
//!   when argv has `{voice}`, `SONARA_LIVE_COMMAND_VOICE` (else the first of
//!   `options.voices`; with `options.voices`, every listed voice is tried)
use serde_json::{json, Value};
use sonara_engine::external::keys::{KeyResolver, MemoryStore};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig};
use sonara_engine::Engine;
use std::sync::Arc;

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn engine(profile: &Value) -> External {
    let p = Profile::from_json(profile).expect("a valid profile");
    External::new(ExternalConfig::new(
        p,
        KeyResolver::new(Arc::new(MemoryStore::new())),
    ))
    .unwrap()
}

/// The profile with a model and a voice: `SONARA_LIVE_<name>_MODEL` and
/// `_VOICE`, else the first the provider lists. `None` (skipped) when one
/// is needed and none is found.
fn complete(name: &str, mut profile: Value) -> Option<Value> {
    let p = Profile::from_json(&profile).expect("a valid profile");
    let e = engine(&profile);
    if p.takes_model() && p.model.is_none() {
        let pick = var(&format!("SONARA_LIVE_{name}_MODEL")).or_else(|| {
            let models = e.refresh_models().expect("the model list");
            let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
            println!("{} models: {ids:?}", models.len());
            models.first().map(|m| m.id.clone())
        });
        match pick {
            Some(m) => profile["model"] = json!(m),
            None if p.model_required() => {
                println!("skipped: no model listed; set SONARA_LIVE_{name}_MODEL");
                return None;
            }
            None => {}
        }
    }
    if p.voice_required() && p.voice.is_none() {
        let pick = var(&format!("SONARA_LIVE_{name}_VOICE")).or_else(|| {
            let voices = e.refresh_voices().expect("the voice list");
            println!("{} voices", voices.len());
            voices.first().map(|v| v.id.clone())
        });
        let Some(v) = pick else {
            println!("skipped: no voice list; set SONARA_LIVE_{name}_VOICE");
            return None;
        };
        profile["voice"] = json!(v);
    }
    Some(profile)
}

fn check(name: &str, profile: Value) {
    let Some(profile) = complete(name, profile) else {
        return;
    };
    let e = engine(&profile);
    let t = e
        .test("Hello. This is a live check of Sonara.", "", 250)
        .expect("one synthesis");
    let samples: usize = t.pcm.iter().map(|c| c.samples.len()).sum();
    let rate = t.pcm[0].sample_rate;
    println!(
        "model {} voice {} in {} ms: {samples} samples at {rate} Hz",
        profile["model"], t.voice, t.ms
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
    check(
        "OPENAI",
        json!({"id": "openai-live", "kind": "openai-compatible",
        "key_ref": "env:OPENAI_API_KEY", "options": {"preset": "openai"}}),
    );
}

#[test]
#[ignore]
fn kokoro_fastapi_live() {
    let Some(url) = var("SONARA_LIVE_KOKORO_FASTAPI_URL") else {
        println!("skipped: set SONARA_LIVE_KOKORO_FASTAPI_URL");
        return;
    };
    check(
        "KOKORO_FASTAPI",
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
        "LOCALAI",
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
        "SPEACHES",
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
    check(
        "ELEVENLABS",
        json!({"id": "elevenlabs-live", "kind": "elevenlabs",
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
        "AZURE",
        json!({"id": "azure-live", "kind": "azure",
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
        "GOOGLE",
        json!({"id": "google-live", "kind": "google",
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
    check(
        "GEMINI",
        json!({"id": "gemini-live", "kind": "gemini", "key_ref": "env:GEMINI_API_KEY"}),
    );
}

#[test]
#[ignore]
fn cartesia_live() {
    if var("CARTESIA_API_KEY").is_none() {
        println!("skipped: set CARTESIA_API_KEY (and SONARA_LIVE_CARTESIA_MODEL)");
        return;
    }
    check(
        "CARTESIA",
        json!({"id": "cartesia-live", "kind": "cartesia", "key_ref": "env:CARTESIA_API_KEY"}),
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
    let Some(profile) = complete(
        "DEEPGRAM",
        json!({"id": "deepgram-live", "kind": "deepgram", "key_ref": "env:DEEPGRAM_API_KEY"}),
    ) else {
        return;
    };
    check("DEEPGRAM", profile.clone());
    let e = engine(&profile);
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
    let mut profile = json!({"id": "command-live", "kind": "command", "options": options});
    // `{voice}` in argv (#274): the voice is SONARA_LIVE_COMMAND_VOICE, else
    // the first of options.voices, so the substitution runs live too; each
    // listed voice is then tried, and one outside the list must be refused.
    let voice_in_argv = profile["options"]["argv"].as_array().is_some_and(|a| {
        a.iter()
            .skip(1)
            .any(|x| x.as_str().is_some_and(|x| x.contains("{voice}")))
    });
    let listed: Vec<String> = profile["options"]["voices"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if voice_in_argv {
        if let Some(v) = var("SONARA_LIVE_COMMAND_VOICE").or_else(|| listed.first().cloned()) {
            profile["voice"] = json!(v);
        } else {
            println!("note: argv has {{voice}} but no voice is set (SONARA_LIVE_COMMAND_VOICE or options.voices)");
        }
    }
    check("COMMAND", profile.clone());
    if voice_in_argv && !listed.is_empty() {
        let e = engine(&profile);
        for v in &listed {
            let t = e
                .test("A voice check.", v, 200)
                .expect("each listed voice speaks");
            let n: usize = t.pcm.iter().map(|c| c.samples.len()).sum();
            println!("voice {v}: {n} samples in {} ms", t.ms);
            assert!(n > 0, "voice {v}: no audio");
        }
        assert!(
            e.test("A voice check.", "sonara-not-a-listed-voice", 200)
                .is_err(),
            "a voice outside options.voices is refused"
        );
    }
}
