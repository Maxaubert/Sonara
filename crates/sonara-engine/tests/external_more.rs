//! Kinds `cartesia`, `deepgram` and `command` (spec 5.4, 13.1, 13.2). The
//! two providers run against a scripted local server; `command` runs the
//! stand-in program `sonara-fake-tts` (feature `test-util`). No real
//! provider and no real speech program is called.
mod common;

use common::{Route, ScriptServer, TempDir};
use serde_json::{json, Value};
use sonara_engine::external::error::cue_text;
use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, Error, PcmChunk, Readiness, Reason};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const KEY: &str = "more-test-key-0123456789";
const VOICE: &str = "a0e99841-438c-4a64-b679-ae501e7d6091";
const FAKE_TTS: &str = env!("CARGO_BIN_EXE_sonara-fake-tts");

fn build(v: Value, key: Option<&str>, fallback: bool) -> External {
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

fn engine(server: &ScriptServer, mut v: Value, key: Option<&str>, fallback: bool) -> External {
    v["url"] = json!(server.base);
    build(v, key, fallback)
}

fn chunks(e: &External, text: &str, voice: &str, rate: u32) -> Vec<PcmChunk> {
    e.synthesize(text, voice, rate)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn failure(r: sonara_engine::Result<impl std::fmt::Debug>) -> (Reason, String) {
    match r {
        Err(Error::External { reason, message }) => (reason, message),
        other => panic!("expected an external error, got {other:?}"),
    }
}

fn cartesia() -> Value {
    json!({"id": "ca", "kind": "cartesia", "voice": VOICE, "options": {"timeout_ms": 5000}})
}

fn deepgram() -> Value {
    json!({"id": "dg", "kind": "deepgram", "voice": "aura-2-thalia-en",
        "options": {"timeout_ms": 5000}})
}

#[test]
fn cartesia_speaks_raw_pcm_with_bearer_and_version() {
    let server = ScriptServer::start();
    server.on(
        "/tts/bytes",
        Route::new(200, "audio/pcm", pcm_bytes(&[-1, 4, 2])),
    );
    let e = engine(&server, cartesia(), Some(KEY), false);
    let got = chunks(&e, "Hello there.", "", 250);
    assert_eq!(
        (got[0].samples.clone(), got[0].sample_rate),
        (vec![-1, 4, 2], 24_000)
    );
    let req = &server.requests()[0];
    assert_eq!(
        (req.method.as_str(), req.path.as_str()),
        ("POST", "/tts/bytes")
    );
    assert_eq!(
        req.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(req.header("cartesia-version"), Some("2026-08-14"));
    assert_eq!(
        req.json(),
        json!({"model_id": "sonic-3.6", "transcript": "Hello there.",
            "voice": {"mode": "id", "id": VOICE},
            "output_format": {"container": "raw", "encoding": "pcm_s16le", "sample_rate": 24000},
            "language": "en", "generation_config": {"speed": 1.25}})
    );
    assert_eq!(e.status().readiness, Readiness::Ready);
}

#[test]
fn cartesia_voice_list_follows_the_cursor() {
    let server = ScriptServer::start();
    server.queue(
        "/voices",
        Route::json(
            200,
            r#"{"data": [{"id": "v1", "name": "Katie", "language": "en"}],
                "has_more": true, "next_page": "v1"}"#,
        ),
    );
    server.queue(
        "/voices",
        Route::json(
            200,
            r#"{"data": [{"id": "v2", "name": "Hans", "language": "de"}], "has_more": false}"#,
        ),
    );
    let e = engine(&server, cartesia(), Some(KEY), false);
    let ids: Vec<String> = e
        .refresh_voices()
        .unwrap()
        .into_iter()
        .map(|v| v.id)
        .collect();
    assert_eq!(ids, vec![VOICE, "v1", "v2"]);
    let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        vec!["/voices?limit=100", "/voices?limit=100&starting_after=v1"]
    );
    assert!(server
        .requests()
        .iter()
        .all(|r| r.header("cartesia-version") == Some("2026-08-14")));
}

#[test]
fn cartesia_unknown_voice_reads_with_the_fallback_and_the_cue() {
    let server = ScriptServer::start();
    server.on(
        "/tts/bytes",
        Route::json(
            404,
            r#"{"error_code": "voice_not_found", "title": "Not Found",
                "message": "Voice not found", "request_id": "r1"}"#,
        ),
    );
    let e = engine(&server, cartesia(), Some(KEY), true);
    let got: Vec<i16> = chunks(&e, "One.", "", 200)
        .into_iter()
        .flat_map(|c| c.samples)
        .collect();
    let cue = cue_text(Reason::BadVoice, "Cartesia");
    assert_eq!(
        cue,
        "Cartesia does not know this voice. Reading with the built-in voice."
    );
    let mut want = FakeEngine::render(&cue, "", 200).unwrap();
    want.extend(FakeEngine::render("One.", "", 200).unwrap());
    assert_eq!(got, want);
    let st = e.status();
    assert_eq!(
        (st.reason, st.readiness),
        (Some(Reason::BadVoice), Readiness::Unavailable)
    );
}

#[test]
fn deepgram_speaks_with_token_auth_and_the_voice_as_model() {
    let server = ScriptServer::start();
    server.on(
        "/v1/speak",
        Route::new(200, "audio/l16", pcm_bytes(&[3, 2, 1])),
    );
    let e = engine(&server, deepgram(), Some(KEY), false);
    let got = chunks(&e, "Hello.", "", 200);
    assert_eq!(
        (got[0].samples.clone(), got[0].sample_rate),
        (vec![3, 2, 1], 24_000)
    );
    let req = &server.requests()[0];
    assert_eq!(
        req.path,
        "/v1/speak?model=aura-2-thalia-en&encoding=linear16&container=none&sample_rate=24000"
    );
    assert_eq!(
        req.header("authorization"),
        Some(format!("Token {KEY}").as_str())
    );
    assert_eq!(req.json(), json!({"text": "Hello."}));
    // Another voice is another model; a rate other than 200 sends speed.
    chunks(&e, "Again.", "aura-2-zeus-en", 300);
    assert_eq!(
        server.requests()[1].path,
        "/v1/speak?model=aura-2-zeus-en&encoding=linear16&container=none\
         &sample_rate=24000&speed=1.5"
    );
}

#[test]
fn deepgram_refused_speed_is_retried_once_without_it_and_remembered() {
    let server = ScriptServer::start();
    server.queue(
        "/v1/speak",
        Route::json(
            400,
            r#"{"err_code": "INVALID_QUERY_PARAMETER", "err_msg": "Unknown parameter: speed"}"#,
        ),
    );
    server.on("/v1/speak", Route::new(200, "audio/l16", pcm_bytes(&[9])));
    let e = engine(&server, deepgram(), Some(KEY), true);
    let got = chunks(&e, "Fast.", "", 250);
    assert_eq!(
        got[0].samples,
        vec![9],
        "the provider spoke, not the fallback"
    );
    chunks(&e, "Still fast.", "", 250);
    let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths.len(), 3, "{paths:?}");
    assert!(paths[0].ends_with("&speed=1.25"));
    assert!(!paths[1].contains("speed") && !paths[2].contains("speed"));
    assert_eq!(e.status().readiness, Readiness::Ready);
}

#[test]
fn deepgram_voices_are_its_tts_models() {
    let server = ScriptServer::start();
    server.on(
        "/v1/models",
        Route::json(
            200,
            r#"{"stt": [{"name": "nova-3", "canonical_name": "nova-3"}],
                "tts": [{"name": "thalia", "canonical_name": "aura-2-thalia-en",
                         "architecture": "aura-2", "languages": ["en"]},
                        {"name": "zeus", "canonical_name": "aura-2-zeus-en",
                         "architecture": "aura-2", "languages": ["en"]}]}"#,
        ),
    );
    let e = engine(&server, deepgram(), Some(KEY), false);
    let voices = e.refresh_voices().unwrap();
    let ids: Vec<&str> = voices.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, vec!["aura-2-thalia-en", "aura-2-zeus-en"]);
    assert_eq!(voices[0].name, "thalia (aura-2)");
    assert_eq!(
        server.requests()[0].header("authorization"),
        Some(format!("Token {KEY}").as_str())
    );
}

#[test]
fn deepgram_refused_key_is_auth_without_the_key_in_the_message() {
    let server = ScriptServer::start();
    server.on(
        "/v1/speak",
        Route::json(
            401,
            r#"{"err_code": "INVALID_AUTH", "err_msg": "Invalid credentials.", "request_id": "r"}"#,
        ),
    );
    let e = engine(&server, deepgram(), Some(KEY), false);
    let (reason, message) = failure(e.test("x", "", 200));
    assert_eq!(reason, Reason::Auth);
    assert_eq!(
        message,
        "Deepgram refused the key (401): Invalid credentials."
    );
}

#[test]
fn cartesia_and_deepgram_without_a_key_never_send_text() {
    for profile in [cartesia(), deepgram()] {
        let server = ScriptServer::start();
        let e = engine(&server, profile.clone(), None, true);
        assert!(e.warm().is_ok(), "a fallback exists");
        chunks(&e, "Private text.", "", 200);
        assert!(server.requests().is_empty(), "{profile}");
        assert_eq!(e.status().reason, Some(Reason::NoKey), "{profile}");
    }
}

// ---- command ----------------------------------------------------------

fn command(argv: Value, options: Value) -> Value {
    let mut o = json!({"argv": argv, "timeout_ms": 5000});
    for (k, v) in options.as_object().unwrap() {
        o[k] = v.clone();
    }
    json!({"id": "cmd", "kind": "command", "options": o})
}

fn record(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("the program wrote its record")).unwrap()
}

#[test]
fn command_reads_stdin_and_prints_a_wav() {
    let dir = TempDir::new("cmd-stdin");
    let rec = dir.path().join("rec.json");
    let e = build(
        command(
            json!([FAKE_TTS, "--record", rec.display().to_string()]),
            json!({}),
        ),
        None,
        false,
    );
    let got = chunks(&e, "Grüße, world.", "", 200);
    assert_eq!(
        (got[0].samples.clone(), got[0].sample_rate),
        (vec![13, 100, -100], 22_050)
    );
    let r = record(&rec);
    assert_eq!(r["stdin"], "Grüße, world.");
    assert_eq!(r["key"], Value::Null, "no key, and none inherited");
    assert_eq!(e.lookahead(), 1);
    assert_eq!(e.status().readiness, Readiness::Ready);
}

#[test]
fn command_arguments_reach_the_program_verbatim_without_a_shell() {
    let dir = TempDir::new("cmd-arg");
    let rec = dir.path().join("rec.json");
    let e = build(
        command(
            json!([
                FAKE_TTS,
                "--text",
                "{text}",
                "--voice={voice}",
                "--wpm",
                "{rate}",
                "--speed",
                "{speed}",
                "--record",
                rec.display().to_string()
            ]),
            json!({"input": "arg", "voices": ["amy", "joe"]}),
        ),
        None,
        false,
    );
    let text = "Fish & chips | echo \"hi\" > x %PATH% ^ $(y)";
    let got = chunks(&e, text, "amy", 250);
    assert_eq!(got[0].samples[0], text.chars().count() as i16);
    let r = record(&rec);
    assert_eq!(
        r["args"],
        json!([
            "--text",
            text,
            "--voice=amy",
            "--wpm",
            "250",
            "--speed",
            "1.25",
            "--record",
            rec.display().to_string()
        ])
    );
    assert_eq!(r["stdin"], "");
    let voices: Vec<String> = e.voices().into_iter().map(|v| v.id).collect();
    assert_eq!(voices, vec!["amy", "joe"]);
    assert_eq!(e.refresh_voices().unwrap().len(), 2);
}

#[test]
fn command_prints_raw_pcm_at_its_rate() {
    let e = build(
        command(
            json!([FAKE_TTS, "--mode", "pcm"]),
            json!({"output": "stdout-pcm",
            "sample_rate": 16000}),
        ),
        None,
        false,
    );
    let got = chunks(&e, "abc", "", 200);
    assert_eq!(
        (got[0].samples.clone(), got[0].sample_rate),
        (vec![3, 100, -100], 16_000)
    );
}

#[test]
fn command_writes_a_file_that_is_removed_afterwards() {
    let dir = TempDir::new("cmd-file");
    let rec = dir.path().join("rec.json");
    let e = build(
        command(
            json!([
                FAKE_TTS,
                "--mode",
                "file",
                "--out",
                "{out}",
                "--rate",
                "44100",
                "--record",
                rec.display().to_string()
            ]),
            json!({"output": "file"}),
        ),
        None,
        false,
    );
    let got = chunks(&e, "Hi.", "", 200);
    assert_eq!(
        (got[0].samples.clone(), got[0].sample_rate),
        (vec![3, 100, -100], 44_100)
    );
    let args = record(&rec)["args"].clone();
    let out = args[3].as_str().unwrap().to_string();
    assert!(
        out.ends_with(".wav") && out.contains("sonara-tts-"),
        "{out}"
    );
    assert!(!Path::new(&out).exists(), "the temporary file is removed");
}

#[test]
fn command_gets_the_key_in_its_environment_only() {
    let dir = TempDir::new("cmd-key");
    let rec = dir.path().join("rec.json");
    let mut v = command(
        json!([FAKE_TTS, "--record", rec.display().to_string()]),
        json!({}),
    );
    v["key_ref"] = json!("credman");
    let e = build(v, Some(KEY), false);
    chunks(&e, "x", "", 200);
    let r = record(&rec);
    assert_eq!(r["key"], KEY);
    assert!(!r["args"].to_string().contains(KEY));
}

#[test]
fn command_failures_map_to_reasons() {
    let run = |args: Value, options: Value| {
        let mut argv = vec![json!(FAKE_TTS)];
        argv.extend(args.as_array().unwrap().iter().cloned());
        failure(build(command(json!(argv), options), None, false).test("Hello.", "", 200))
    };
    // A crash of the user's program is transient: its last stderr line,
    // masked, says why.
    let (reason, message) = run(json!(["--exit", "3"]), json!({}));
    assert_eq!(reason, Reason::Server);
    assert_eq!(
        message,
        "The speech program (sonara-fake-tts.exe) failed (exit code 3): \
         fake-tts: the model broke, key [redacted]"
    );
    let (reason, message) = run(json!(["--mode", "garbage"]), json!({}));
    assert_eq!(reason, Reason::Format);
    assert!(message.contains("not WAV"), "{message}");
    let (reason, message) = run(json!(["--mode", "empty"]), json!({}));
    assert_eq!(reason, Reason::Format);
    assert!(message.ends_with("gave no audio"), "{message}");
    let (reason, message) = run(
        json!(["--mode", "empty", "--out", "{out}"]),
        json!({"output": "file"}),
    );
    assert_eq!(reason, Reason::Format);
    assert!(message.ends_with("wrote no audio file"), "{message}");
    let gone = build(
        command(json!(["C:\\Nope\\sonara-missing-tts.exe"]), json!({})),
        None,
        false,
    );
    assert_eq!(failure(gone.test("x", "", 200)).0, Reason::BadConfig);
}

#[test]
fn a_failing_command_reads_with_the_fallback() {
    let e = build(
        command(json!([FAKE_TTS, "--exit", "1"]), json!({})),
        None,
        true,
    );
    let got: Vec<i16> = chunks(&e, "One.", "", 200)
        .into_iter()
        .flat_map(|c| c.samples)
        .collect();
    let cue = cue_text(Reason::Server, "The speech program");
    let mut want = FakeEngine::render(&cue, "", 200).unwrap();
    want.extend(FakeEngine::render("One.", "", 200).unwrap());
    assert_eq!(got, want);
}

/// Waits until `deadline` and says whether `marker` appeared (the program
/// writes it after its sleep, so a killed program never does).
fn marker_after(marker: &Path, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    marker.exists()
}

#[test]
fn command_over_its_timeout_is_killed() {
    let dir = TempDir::new("cmd-timeout");
    let marker = dir.path().join("marker");
    let e = build(
        command(
            json!([
                FAKE_TTS,
                "--sleep-ms",
                "2500",
                "--marker",
                marker.display().to_string()
            ]),
            json!({"timeout_ms": 1000}),
        ),
        None,
        false,
    );
    let start = Instant::now();
    let (reason, message) = failure(e.test("x", "", 200));
    assert_eq!(reason, Reason::Timeout);
    assert!(message.contains("did not finish in 1 s"), "{message}");
    assert!(
        start.elapsed() < Duration::from_millis(2000),
        "{:?}",
        start.elapsed()
    );
    assert!(
        !marker_after(&marker, start + Duration::from_millis(3500)),
        "the program was killed"
    );
}

#[test]
fn cancel_ends_a_command_at_once_and_kills_it() {
    let dir = TempDir::new("cmd-cancel");
    let marker = dir.path().join("marker");
    let e = Arc::new(build(
        command(
            json!([
                FAKE_TTS,
                "--sleep-ms",
                "2000",
                "--marker",
                marker.display().to_string()
            ]),
            json!({}),
        ),
        None,
        true,
    ));
    let e2 = e.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        e2.cancel();
    });
    let start = Instant::now();
    assert!(matches!(e.synthesize("x", "", 200), Err(Error::Cancelled)));
    assert!(
        start.elapsed() < Duration::from_millis(1000),
        "{:?}",
        start.elapsed()
    );
    assert!(
        !marker_after(&marker, start + Duration::from_millis(3000)),
        "the program was killed"
    );
}
