//! The mute rule of external engines (#227): while a `Hold` is held, no
//! request of any kind reaches the provider (synthesis, lookahead, voice
//! lists); the chunk is spoken with the local fallback instead. Raising the
//! hold ends a request in flight. Only explicit user actions (`test`, and a
//! synthesis inside `hold::explicit`, the voice preview) still reach it.
mod common;

use common::{Route, ScriptServer};
use serde_json::json;
use sonara_engine::external::hold::{self, Hold};
use sonara_engine::external::keys::{KeyResolver, MemoryStore};
use sonara_engine::external::profile::Profile;
use sonara_engine::external::{External, ExternalConfig, Notice};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, Error, Readiness};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SPEECH: &str = "/v1/audio/speech";
const VOICES: &str = "/v1/audio/voices";

struct Rig {
    server: ScriptServer,
    fake: Arc<FakeEngine>,
    engine: Arc<External>,
    hold: Arc<Hold>,
    notices: Arc<Mutex<Vec<Notice>>>,
}

fn rig_with(fallback: bool) -> Rig {
    let server = ScriptServer::start();
    let fake = Arc::new(FakeEngine::new());
    let profile = Profile::from_json(&json!({"id": "hold-test", "kind": "openai-compatible",
        "label": "Test", "voice": "v1", "url": format!("{}/v1", server.base),
        "options": {"preset": "kokoro-fastapi", "timeout_ms": 5000}}))
    .unwrap();
    let mut config = ExternalConfig::new(profile, KeyResolver::new(Arc::new(MemoryStore::new())));
    if fallback {
        config.fallback = Some(fake.clone() as Arc<dyn Engine>);
    }
    let hold = Arc::new(Hold::new());
    config.hold = Some(hold.clone());
    let notices = Arc::new(Mutex::new(Vec::new()));
    let n = notices.clone();
    config.notice = Some(Arc::new(move |x| n.lock().unwrap().push(x)));
    server.on(SPEECH, Route::wav(&[1, 2, 3], 24_000));
    server.on(
        VOICES,
        Route::json(200, r#"{"voices": ["af_heart", "am_echo"]}"#),
    );
    Rig {
        server,
        fake,
        engine: Arc::new(External::new(config).unwrap()),
        hold,
        notices,
    }
}

fn rig() -> Rig {
    rig_with(true)
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
fn a_held_engine_speaks_with_the_fallback_and_sends_nothing() {
    let r = rig();
    assert!(r.hold.set(true), "a change");
    assert!(!r.hold.set(true), "no change");
    assert_eq!(samples(&r.engine, "Hello.").unwrap(), fake_audio("Hello."));
    assert_eq!(r.server.requests().len(), 0, "no request while held");
    // Not a failure: no cue, no notice, and the engine stays ready.
    assert_eq!(r.fake.texts(), vec!["Hello.".to_string()]);
    assert!(r.notices.lock().unwrap().is_empty());
    assert_eq!(r.engine.status().readiness, Readiness::Ready);
}

#[test]
fn requests_resume_once_the_hold_is_released() {
    let r = rig();
    r.hold.set(true);
    samples(&r.engine, "Muted.").unwrap();
    assert_eq!(r.server.count(SPEECH), 0);
    assert!(r.hold.set(false));
    assert_eq!(samples(&r.engine, "Back.").unwrap(), vec![1, 2, 3]);
    assert_eq!(r.server.count(SPEECH), 1);
}

#[test]
fn raising_the_hold_ends_a_request_in_flight_and_speaks_it_locally() {
    let r = rig();
    r.server.on(
        SPEECH,
        Route::wav(&[1], 24_000).delayed(Duration::from_secs(3)),
    );
    let engine = r.engine.clone();
    let speaker = std::thread::spawn(move || {
        let got = samples(&engine, "Slow one.");
        (got, Instant::now())
    });
    // Raise the hold once the request is in flight (the server has it), not
    // after a fixed sleep: on a busy machine the request can take longer to
    // arrive (#256).
    let deadline = Instant::now() + Duration::from_secs(5);
    while r.server.count(SPEECH) == 0 {
        assert!(Instant::now() < deadline, "the request never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
    let raised_at = Instant::now();
    r.hold.set(true);
    let (got, ended) = speaker.join().unwrap();
    assert_eq!(got.unwrap(), fake_audio("Slow one."));
    assert!(
        ended.duration_since(raised_at) < Duration::from_millis(300),
        "ended {:?} after the hold",
        ended.duration_since(raised_at)
    );
    assert_eq!(r.server.count(SPEECH), 1, "only the request already sent");
    samples(&r.engine, "Next.").unwrap();
    assert_eq!(r.server.count(SPEECH), 1, "nothing more while held");
}

#[test]
fn a_chunk_begun_before_the_hold_is_spoken_locally() {
    // The reader calls `begin` (lookahead), then the user mutes before the
    // chunk is synthesized: it never reaches the provider.
    let r = rig();
    r.engine.begin();
    r.hold.set(true);
    assert_eq!(samples(&r.engine, "Ahead.").unwrap(), fake_audio("Ahead."));
    assert_eq!(r.server.count(SPEECH), 0);
}

#[test]
fn voice_lists_are_not_fetched_while_held() {
    let r = rig();
    r.hold.set(true);
    let listed = r.engine.refresh_voices().unwrap();
    assert!(listed.iter().all(|v| v.id != "am_echo"), "{listed:?}");
    assert_eq!(r.server.requests().len(), 0);
    r.hold.set(false);
    let listed = r.engine.refresh_voices().unwrap();
    assert!(listed.iter().any(|v| v.id == "am_echo"));
    assert_eq!(r.server.count(VOICES), 1);
}

#[test]
fn a_held_engine_without_a_fallback_fails_without_a_request() {
    let r = rig_with(false);
    r.hold.set(true);
    assert!(samples(&r.engine, "Nothing.").is_err());
    assert_eq!(r.server.requests().len(), 0);
}

#[test]
fn explicit_user_actions_still_reach_the_provider() {
    let r = rig();
    r.hold.set(true);
    // The Test button.
    let t = r.engine.test("Testing.", "", 200).unwrap();
    assert_eq!(t.pcm[0].samples, vec![1, 2, 3]);
    assert_eq!(r.server.count(SPEECH), 1);
    // A voice preview the user asked for.
    let got = hold::explicit(|| samples(&r.engine, "Preview.")).unwrap();
    assert_eq!(got, vec![1, 2, 3]);
    assert_eq!(r.server.count(SPEECH), 2);
    // Outside the scope the hold applies again.
    samples(&r.engine, "Held.").unwrap();
    assert_eq!(r.server.count(SPEECH), 2);
}

#[test]
fn a_local_scope_uses_the_fallback_even_unheld() {
    // Sonara's own mute cues ("Unmuted.") are spoken locally.
    let r = rig();
    let got = hold::local(|| samples(&r.engine, "Unmuted.")).unwrap();
    assert_eq!(got, fake_audio("Unmuted."));
    assert_eq!(r.server.requests().len(), 0);
    assert_eq!(samples(&r.engine, "Normal.").unwrap(), vec![1, 2, 3]);
}

#[test]
fn an_engine_without_a_hold_is_unaffected() {
    let server = ScriptServer::start();
    server.on(SPEECH, Route::wav(&[9], 24_000));
    let profile = Profile::from_json(&json!({"id": "free", "kind": "openai-compatible",
        "voice": "v1", "url": format!("{}/v1", server.base), "options": {"preset": "kokoro-fastapi"}}))
    .unwrap();
    let engine = External::new(ExternalConfig::new(
        profile,
        KeyResolver::new(Arc::new(MemoryStore::new())),
    ))
    .unwrap();
    assert_eq!(samples(&engine, "Free.").unwrap(), vec![9]);
}

#[test]
fn a_mute_between_taking_the_generation_and_the_check_sends_nothing() {
    // The hold lands after `synthesize` took its cancel generation but
    // before it looked at the hold: the request must still not be sent.
    let r = rig();
    let hold = r.hold.clone();
    let got = hold::with_race_hook(
        move || {
            hold.set(true);
        },
        || samples(&r.engine, "Raced."),
    );
    assert_eq!(got.unwrap(), fake_audio("Raced."));
    assert_eq!(r.server.requests().len(), 0, "nothing sent once muted");
}

#[test]
fn a_mute_and_unmute_within_one_request_still_speaks_the_chunk() {
    // "Nothing may silently drop it": the cut request is spoken locally,
    // not reported as a failed chunk, even though the hold lifted again.
    let r = rig();
    r.server.on(
        SPEECH,
        Route::wav(&[1], 24_000).delayed(Duration::from_secs(3)),
    );
    let hold = r.hold.clone();
    let cycler = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        hold.set(true);
        hold.set(false);
    });
    let got = samples(&r.engine, "Cycled.");
    cycler.join().unwrap();
    assert_eq!(got.unwrap(), fake_audio("Cycled."));
    assert_eq!(r.server.count(SPEECH), 1, "only the request already sent");
}

#[test]
fn a_chunk_begun_before_a_mute_and_unmute_is_still_spoken() {
    let r = rig();
    r.engine.begin();
    r.hold.set(true);
    r.hold.set(false);
    assert_eq!(samples(&r.engine, "Ahead.").unwrap(), fake_audio("Ahead."));
    assert_eq!(r.server.count(SPEECH), 0);
    // A reader cancel alone is still a cancel.
    r.engine.begin();
    r.engine.cancel();
    assert!(matches!(
        samples(&r.engine, "Skipped."),
        Err(Error::Cancelled)
    ));
}

#[test]
fn a_mute_during_a_paged_voice_list_stops_the_pages() {
    use sonara_engine::external::keys::{KeyStore, Secret};
    let server = ScriptServer::start();
    let page1 = r#"{"voices": [{"voice_id": "a", "name": "A"}], "has_more": true,
        "next_page_token": "p2"}"#;
    let page2 = r#"{"voices": [{"voice_id": "b", "name": "B"}], "has_more": false}"#;
    server.queue(
        "/v2/voices",
        Route::json(200, page1).delayed(Duration::from_millis(400)),
    );
    server.queue("/v2/voices", Route::json(200, page2));
    let profile = Profile::from_json(&json!({"id": "el", "kind": "elevenlabs",
        "url": server.base, "voice": "voiceid0000000000001",
        "options": {"timeout_ms": 5000}}))
    .unwrap();
    let store = Arc::new(MemoryStore::new());
    store
        .set(
            "el",
            &Secret::new("cloud-test-key-0123456789"),
            &profile.origin().unwrap(),
        )
        .unwrap();
    let mut config = ExternalConfig::new(profile, KeyResolver::new(store));
    let hold = Arc::new(Hold::new());
    config.hold = Some(hold.clone());
    let engine = External::new(config).unwrap();
    let h = hold.clone();
    let raiser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        h.set(true);
    });
    let listed = engine.refresh_voices().unwrap();
    raiser.join().unwrap();
    assert_eq!(server.requests().len(), 1, "no page after the mute");
    assert!(listed.iter().all(|v| v.id != "b"), "{listed:?}");
    assert!(engine.voices_stale(), "a cut list is not kept as fresh");
    // Unmuted, the whole list is fetched again.
    hold.set(false);
    server.queue("/v2/voices", Route::json(200, page1));
    server.queue("/v2/voices", Route::json(200, page2));
    let listed = engine.refresh_voices().unwrap();
    assert!(listed.iter().any(|v| v.id == "b"), "{listed:?}");
}
