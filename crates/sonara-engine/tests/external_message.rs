//! Send mode `message` (#235) against a scripted local server: a whole
//! message is one request; its raw PCM plays from the first piece
//! (ElevenLabs' `/stream`); a replay (Up) is served from the kept audio
//! without a request; a cancel mid-stream ends the audio, closes the
//! connection and sends nothing more; a failure before any audio reads the
//! whole message with the fallback (one cue, one notice); an answer cut
//! after audio keeps that audio (one notice, no fallback, not kept for a
//! replay). Sentence mode (`send_mode: sentence`) keeps one request per
//! text and reads its answer whole. No real provider is called.
mod common;

use common::{Route, ScriptServer};
use serde_json::{json, Value};
use sonara_engine::external::error::cue_text;
use sonara_engine::external::hold::Hold;
use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig, Notice};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, Error, InputLimit, Reason, SendMode};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const KEY: &str = "cloud-test-key-0123456789";
const VOICE: &str = "voiceid0000000000001";
const STREAM: &str = "/v1/text-to-speech/voiceid0000000000001/stream";
const WHOLE: &str = "/v1/text-to-speech/voiceid0000000000001";

/// A reply of several sentences in two paragraphs (longer than a cue).
const MESSAGE: &str = "The build passed on the first try. All tests are green.\n\nThe release goes out tonight. Nothing else is needed.";

struct Rig {
    server: ScriptServer,
    engine: External,
    notices: Arc<Mutex<Vec<Notice>>>,
}

fn rig(v: Value) -> Rig {
    rig_with(v, None)
}

fn rig_held(v: Value, hold: Arc<Hold>) -> Rig {
    rig_with(v, Some(hold))
}

fn rig_with(mut v: Value, hold: Option<Arc<Hold>>) -> Rig {
    let server = ScriptServer::start();
    v["url"] = json!(server.base);
    let store = Arc::new(MemoryStore::new());
    let profile = Profile::from_json(&v).unwrap();
    store
        .set("el", &Secret::new(KEY), &profile.origin().unwrap())
        .unwrap();
    let mut config = ExternalConfig::new(profile, KeyResolver::new(store));
    config.fallback = Some(Arc::new(FakeEngine::new()) as Arc<dyn Engine>);
    config.hold = hold;
    let notices: Arc<Mutex<Vec<Notice>>> = Arc::default();
    let n = notices.clone();
    config.notice = Some(Arc::new(move |x| n.lock().unwrap().push(x)));
    Rig {
        engine: External::new(config).unwrap(),
        server,
        notices,
    }
}

fn elevenlabs(extra: Value) -> Value {
    let mut v = json!({"id": "el", "kind": "elevenlabs", "voice": VOICE,
        "options": {"timeout_ms": 5000, "first_audio_ms": 2000}});
    if let Value::Object(m) = extra {
        for (k, x) in m {
            v[k] = x;
        }
    }
    v
}

fn pcm(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// Raw PCM in pieces: `n` pieces of two samples, `every` apart.
fn pieces(n: usize, every: Duration) -> Route {
    Route::raw(
        "application/octet-stream",
        (0..n)
            .map(|i| (every, pcm(&[i as i16, -(i as i16)])))
            .collect(),
    )
}

fn all(e: &External, text: &str) -> Vec<i16> {
    e.synthesize(text, "", 200)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .flat_map(|c| c.samples)
        .collect()
}

#[test]
fn a_cloud_profile_takes_whole_messages_by_default() {
    let r = rig(elevenlabs(json!({})));
    assert_eq!(r.engine.send_mode(), SendMode::Message);
    assert!(r.engine.streams(), "a message plays as it comes");
    assert_eq!(r.engine.input_limit(), InputLimit::Chars(5000));
    let own = rig(elevenlabs(json!({"options": {"chunk_chars": 800}})));
    assert_eq!(own.engine.input_limit(), InputLimit::Chars(800));
    let s = rig(elevenlabs(json!({"send_mode": "sentence"})));
    assert_eq!(s.engine.send_mode(), SendMode::Sentence);
    assert!(!s.engine.streams());
}

#[test]
fn a_message_is_one_request_that_plays_from_its_first_piece() {
    let r = rig(elevenlabs(json!({})));
    // The first piece at once, the rest after 600 ms.
    let mut route = pieces(1, Duration::ZERO);
    route
        .pieces
        .push((Duration::from_millis(600), pcm(&[7, 8, 9])));
    r.server.on(STREAM, route);
    let start = Instant::now();
    let mut stream = r.engine.synthesize(MESSAGE, "", 200).unwrap();
    let first = stream.next().unwrap().unwrap();
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "the first piece plays before the rest is made: {:?}",
        start.elapsed()
    );
    assert_eq!(first.samples, vec![0, 0]);
    assert_eq!(first.sample_rate, 24_000);
    let rest: Vec<i16> = stream
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .flat_map(|c| c.samples)
        .collect();
    assert_eq!(rest, vec![7, 8, 9]);
    let reqs = r.server.requests();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0]
        .path
        .starts_with(&format!("{STREAM}?output_format=pcm_24000")));
    assert_eq!(
        reqs[0].json()["text"],
        MESSAGE,
        "the whole text, paragraphs kept"
    );
    assert_eq!(reqs[0].header("xi-api-key"), Some(KEY));
}

/// Up (Restart) reads the message again as a new item: the audio kept from
/// the complete answer plays, no request is sent (nothing more billed).
#[test]
fn a_replay_of_a_whole_message_sends_no_request() {
    let r = rig(elevenlabs(json!({})));
    r.server.on(STREAM, pieces(3, Duration::ZERO));
    let first = all(&r.engine, MESSAGE);
    assert_eq!(all(&r.engine, MESSAGE), first);
    assert_eq!(r.server.count(STREAM), 1);
    // In sentence mode a long text is not kept: a replay asks again.
    let s = rig(elevenlabs(json!({"send_mode": "sentence"})));
    s.server.on(
        WHOLE,
        Route::new(200, "application/octet-stream", pcm(&[1, 2])),
    );
    all(&s.engine, MESSAGE);
    all(&s.engine, MESSAGE);
    assert_eq!(s.server.count(WHOLE), 2);
    assert_eq!(
        s.server.count(STREAM),
        0,
        "sentence mode waits for the whole answer"
    );
}

/// Skip, flush, mute or a new turn end the item, and the reader cancels
/// the engine: no further audio, the connection is closed, nothing more is
/// asked for.
#[test]
fn a_cancel_mid_stream_ends_the_audio_and_closes_the_connection() {
    let r = rig(elevenlabs(json!({})));
    r.server.on(STREAM, pieces(300, Duration::from_millis(10)));
    r.engine.begin();
    let mut stream = r.engine.synthesize(MESSAGE, "", 200).unwrap();
    assert!(stream.next().unwrap().is_ok());
    r.engine.cancel();
    let next = stream.next();
    assert!(matches!(next, Some(Err(Error::Cancelled))), "{next:?}");
    assert!(stream.next().is_none(), "no further audio");
    let end = Instant::now() + Duration::from_secs(5);
    let done = loop {
        if let Some(s) = r.server.streamed().pop() {
            break s;
        }
        assert!(Instant::now() < end, "the server never saw the end");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(done.cut, "the client closed the connection: {done:?}");
    assert!(done.written < 300, "{done:?}");
    assert_eq!(r.server.count(STREAM), 1, "nothing more was asked for");
    assert!(
        r.notices.lock().unwrap().is_empty(),
        "a cancel is no failure"
    );
}

/// A failure before any audio: the fallback reads the whole message, the
/// cue first, with one notice for the one request.
#[test]
fn a_failed_message_reads_whole_with_the_fallback() {
    let r = rig(elevenlabs(json!({})));
    r.server.on(
        STREAM,
        Route::json(500, r#"{"detail": {"message": "boom"}}"#),
    );
    let got = all(&r.engine, MESSAGE);
    let mut want = FakeEngine::render(&cue_text(Reason::Server, "ElevenLabs"), "", 200).unwrap();
    want.extend(FakeEngine::render(MESSAGE, "", 200).unwrap());
    assert_eq!(got, want);
    let n = r.notices.lock().unwrap().clone();
    assert_eq!(n.len(), 1, "one fallback line per request: {n:?}");
    assert_eq!(n[0].reason, Some(Reason::Server));
    assert_eq!(r.server.count(STREAM), 1);
}

/// No audio within `first_audio_ms`: the fallback reads the message.
#[test]
fn no_first_audio_in_time_reads_with_the_fallback() {
    let r = rig(elevenlabs(
        json!({"options": {"timeout_ms": 5000, "first_audio_ms": 1000}}),
    ));
    r.server.on(STREAM, pieces(2, Duration::from_millis(1500)));
    let start = Instant::now();
    let got = all(&r.engine, MESSAGE);
    assert!(
        start.elapsed() < Duration::from_millis(1400),
        "{:?}",
        start.elapsed()
    );
    assert!(got.len() > FakeEngine::render(MESSAGE, "", 200).unwrap().len());
    assert_eq!(r.notices.lock().unwrap()[0].reason, Some(Reason::Timeout));
}

/// The audio of `secs` seconds at ElevenLabs' 24 kHz.
fn seconds(secs: f64) -> Vec<u8> {
    pcm(&vec![1i16; (secs * 24_000.0) as usize])
}

/// The fallback's reading of `text`, the cue first when `cue` names it.
fn local(text: &str, cue: Option<Reason>) -> Vec<i16> {
    let mut want = match cue {
        Some(r) => FakeEngine::render(&cue_text(r, "ElevenLabs"), "", 200).unwrap(),
        None => Vec::new(),
    };
    want.extend(FakeEngine::render(text, "", 200).unwrap());
    want
}

/// What follows the first sentence of `MESSAGE` (35 characters with its
/// space): the estimated boundary after 3.5 s of audio, at 12 characters
/// a second, is in the second sentence, so the rest starts there.
const AFTER_FIRST: &str = "All tests are green.

The release goes out tonight. Nothing else is needed.";

/// A long answer that streams steadily is never cut by `timeout_ms` (review
/// of #236): the timeout is a stall limit between pieces, and the whole
/// answer may take as long as its text needs. It is kept for a replay.
#[test]
fn a_long_answer_streaming_steadily_past_timeout_ms_is_not_cut() {
    let r = rig(elevenlabs(
        json!({"options": {"timeout_ms": 1000, "first_audio_ms": 1000}}),
    ));
    r.server.on(STREAM, pieces(25, Duration::from_millis(100)));
    let start = Instant::now();
    let got = all(&r.engine, MESSAGE);
    assert!(start.elapsed() > Duration::from_millis(2000));
    assert_eq!(got.len(), 50, "every piece played");
    assert!(r.notices.lock().unwrap().is_empty());
    all(&r.engine, MESSAGE);
    assert_eq!(r.server.count(STREAM), 1, "complete, so kept for a replay");
}

/// An answer that stalls after audio came (no audio for `first_audio_ms`):
/// the audio received stays, and the rest of the text is read with the
/// fallback from the start of the sentence the audio had reached (an
/// estimate that errs towards reading a little twice, never towards
/// dropping). One notice; the cut answer is not kept for a replay.
#[test]
fn a_stalled_answer_reads_the_rest_with_the_fallback_from_a_sentence() {
    let r = rig(elevenlabs(
        json!({"options": {"timeout_ms": 5000, "first_audio_ms": 1000}}),
    ));
    let mut route = Route::raw(
        "application/octet-stream",
        vec![(Duration::ZERO, seconds(3.5))],
    );
    route.pieces.push((Duration::from_millis(2500), pcm(&[5])));
    r.server.on(STREAM, route);
    let got = all(&r.engine, MESSAGE);
    let played = (3.5 * 24_000.0) as usize;
    assert_eq!(&got[..played], &vec![1i16; played][..]);
    assert_eq!(got[played..], local(AFTER_FIRST, Some(Reason::Timeout))[..]);
    let n = r.notices.lock().unwrap().clone();
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].reason, Some(Reason::Timeout));
    assert_eq!(
        n[0].fallback,
        Some(FakeEngine::new().id()),
        "the rest is read"
    );
    r.server.on(STREAM, pieces(2, Duration::ZERO));
    all(&r.engine, MESSAGE);
    assert_eq!(r.server.count(STREAM), 2, "a cut answer is not replayed");
}

/// A connection that breaks after audio came: as a stall, the rest is
/// read with the fallback from the sentence reached.
#[test]
fn a_broken_answer_reads_the_rest_with_the_fallback() {
    let r = rig(elevenlabs(json!({})));
    let route = Route::raw(
        "application/octet-stream",
        vec![(Duration::ZERO, seconds(0.5))],
    );
    // More promised than sent: the connection breaks after the audio.
    r.server
        .on(STREAM, route.header("Content-Length", "10000000"));
    let got = all(&r.engine, MESSAGE);
    let played = 12_000;
    // 0.5 s is in the first sentence: all of it is read again.
    assert_eq!(
        got[played..],
        local(MESSAGE, Some(Reason::Network))[..],
        "{}",
        got.len()
    );
    assert_eq!(r.notices.lock().unwrap().len(), 1);
}

/// A mute during a streamed message (the hold) ends the provider's answer,
/// but not the message: the rest is read locally from the sentence the
/// audio had reached, with no cue and no notice (a mute is no failure).
#[test]
fn a_mute_mid_message_reads_the_rest_locally() {
    let hold = Arc::new(Hold::new());
    let r = rig_held(elevenlabs(json!({})), hold.clone());
    let mut route = Route::raw(
        "application/octet-stream",
        vec![(Duration::ZERO, seconds(3.5))],
    );
    for _ in 0..50 {
        route.pieces.push((Duration::from_millis(50), pcm(&[5])));
    }
    r.server.on(STREAM, route);
    r.engine.begin();
    let mut stream = r.engine.synthesize(MESSAGE, "", 200).unwrap();
    let mut got: Vec<i16> = Vec::new();
    while got.len() < (3.5 * 24_000.0) as usize {
        got.extend(stream.next().unwrap().unwrap().samples);
    }
    hold.set(true);
    let rest: Vec<i16> = stream
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .flat_map(|c| c.samples)
        .collect();
    assert_eq!(rest, local(AFTER_FIRST, None));
    assert!(r.notices.lock().unwrap().is_empty());
}

/// A body that is WAV after all (a server that ignores the raw format) is
/// read whole.
#[test]
fn a_wav_answer_to_a_raw_request_is_read_whole() {
    let r = rig(elevenlabs(json!({})));
    r.server.on(STREAM, Route::wav(&[3, 4, 5], 22_050));
    let got = r
        .engine
        .synthesize(MESSAGE, "", 200)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(got[0].samples, vec![3, 4, 5]);
    assert_eq!(got[0].sample_rate, 22_050);
}

/// OpenAI's `pcm` and Cartesia's raw bytes are read as they come too; a
/// server answering WAV (the OpenAI-compatible default) is waited for.
#[test]
fn openai_pcm_and_cartesia_stream_in_message_mode() {
    for (v, path) in [
        (
            json!({"id": "el", "kind": "openai-compatible", "model": "m", "voice": "v",
                "send_mode": "message",
                "options": {"preset": "generic", "response_format": "pcm", "timeout_ms": 5000}}),
            "/v1/audio/speech",
        ),
        (
            json!({"id": "el", "kind": "cartesia", "model": "m", "voice": "v",
                "options": {"timeout_ms": 5000}}),
            "/tts/bytes",
        ),
    ] {
        let mut v = v;
        let r = {
            let server = ScriptServer::start();
            v["url"] = json!(if path.starts_with("/v1") {
                format!("{}/v1", server.base)
            } else {
                server.base.clone()
            });
            let store = Arc::new(MemoryStore::new());
            let profile = Profile::from_json(&v).unwrap();
            if let Some(o) = profile.origin() {
                store.set("el", &Secret::new(KEY), &o).unwrap();
            }
            let mut config = ExternalConfig::new(profile, KeyResolver::new(store));
            config.fallback = Some(Arc::new(FakeEngine::new()) as Arc<dyn Engine>);
            Rig {
                engine: External::new(config).unwrap(),
                server,
                notices: Arc::default(),
            }
        };
        let mut route = pieces(1, Duration::ZERO);
        route.pieces.push((Duration::from_millis(600), pcm(&[9])));
        r.server.on(path, route);
        let start = Instant::now();
        let mut stream = r.engine.synthesize(MESSAGE, "", 200).unwrap();
        assert_eq!(
            stream.next().unwrap().unwrap().samples,
            vec![0, 0],
            "{path}"
        );
        assert!(start.elapsed() < Duration::from_millis(500), "{path}");
        let rest: Vec<_> = stream.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(rest[0].samples, vec![9], "{path}");
        assert_eq!(r.server.count(path), 1, "{path}");
    }
}
