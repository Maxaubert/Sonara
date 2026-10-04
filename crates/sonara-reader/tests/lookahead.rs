//! The worker asks the engine for its prefetch depth (spec D10) and lets an
//! engine that accepts unlisted voices take any voice id; a replaced engine
//! applies on the next `set engine`.
mod common;

use common::engines::{GateEngine, OpenEngine};
use common::{Rig, THREE, TIMEOUT};
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config, Error, Key, QueueMode, Registry, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + TIMEOUT;
    while !ok() {
        assert!(Instant::now() < end, "waited for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_worker_prefetches_as_deep_as_the_engine_asks() {
    let engine = Arc::new(OpenEngine::new("open", 2));
    let r = Rig::with(engine.clone());
    r.h.speak(THREE, QueueMode::Append, false, None).unwrap();
    // The playing chunk and two ahead, before any audio finished.
    wait_for("three syntheses", || engine.inner.syntheses() == 3);
}

#[test]
fn switching_engines_sets_the_new_engines_depth() {
    let gate = Arc::new(GateEngine::default());
    let open = Arc::new(OpenEngine::new("open", 3));
    let registry = Registry::default();
    registry.register(gate.clone()).unwrap();
    registry.register(open.clone()).unwrap();
    let r = Rig::config(Config::new(registry));
    r.h.set(Key::Engine, Value::Text("open".into())).unwrap();
    r.h.speak(
        "One. Two. Three. Four. Five.",
        QueueMode::Append,
        false,
        None,
    )
    .unwrap();
    wait_for("four syntheses", || open.inner.syntheses() == 4);
    assert_eq!(gate.started(), 0);
}

#[test]
fn unlisted_voice_accepted_by_open_engine() {
    let engine = Arc::new(OpenEngine::new("open", 1));
    let r = Rig::with(engine.clone());
    r.h.set(Key::Voice, Value::Text("my-cloned-voice".into()))
        .unwrap();
    assert_eq!(
        r.h.get(Key::Voice).unwrap(),
        Value::Text("my-cloned-voice".into())
    );
    r.h.speak("Hi.", QueueMode::Append, false, None).unwrap();
    wait_for("a synthesis", || engine.inner.syntheses() == 1);
    assert_eq!(
        engine.voices_seen.lock().unwrap().clone(),
        vec!["my-cloned-voice"]
    );
}

#[test]
fn unlisted_voice_refused_by_a_listed_only_engine() {
    // Kokoro and OneCore list their voices; the fake engine stands in.
    let (r, _) = Rig::new();
    assert_eq!(
        r.h.set(Key::Voice, Value::Text("my-cloned-voice".into())),
        Err(Error::Engine(sonara_engine::Error::UnknownVoice(
            "my-cloned-voice".into()
        )))
    );
}

#[test]
fn a_replaced_engine_applies_on_the_next_set_engine() {
    let registry = Arc::new(Registry::default());
    registry.register(Arc::new(FakeEngine::new())).unwrap();
    let first = Arc::new(OpenEngine::new("open", 1));
    registry.register(first.clone()).unwrap();
    let r = Rig::config(Config::new(registry.clone()));
    assert!(Arc::ptr_eq(&r.h.registry(), &registry));
    r.h.set(Key::Engine, Value::Text("open".into())).unwrap();
    r.h.set(Key::Voice, Value::Text("v1".into())).unwrap();
    let second = Arc::new(OpenEngine::new("open", 1));
    registry.replace(second.clone()).unwrap();
    // The same id again swaps to the new instance and keeps the voice.
    r.h.set(Key::Engine, Value::Text("open".into())).unwrap();
    assert_eq!(r.h.get(Key::Voice).unwrap(), Value::Text("v1".into()));
    r.h.speak("Hi.", QueueMode::Append, false, None).unwrap();
    wait_for("the new instance", || second.inner.syntheses() == 1);
    assert_eq!(first.inner.syntheses(), 0);
}

#[test]
fn the_worker_joins_sentences_for_an_engine_that_asks() {
    // #235: an engine billed per request (Gemini) gets longer chunks; the
    // first stays one sentence. Switching back to an engine that asks for
    // none reads one sentence per chunk again.
    let joined = Arc::new(OpenEngine {
        chunk_chars: 1000,
        ..OpenEngine::new("joined", 1)
    });
    let plain = Arc::new(OpenEngine::new("plain", 1));
    let registry = Registry::default();
    registry.register(plain.clone()).unwrap();
    registry.register(joined.clone()).unwrap();
    let r = Rig::config(Config::new(registry));
    r.h.set(Key::Engine, Value::Text("joined".into())).unwrap();
    r.h.speak("One. Two. Three.", QueueMode::Append, false, None)
        .unwrap();
    wait_for("two syntheses", || joined.inner.syntheses() == 2);
    assert_eq!(joined.inner.texts(), vec!["One.", "Two. Three."]);
    r.h.set(Key::Engine, Value::Text("plain".into())).unwrap();
    r.h.speak("Four. Five.", QueueMode::Replace, true, None)
        .unwrap();
    wait_for("one sentence each", || plain.inner.syntheses() == 2);
    assert_eq!(plain.inner.texts(), vec!["Four.", "Five."]);
}

#[test]
fn the_worker_sends_a_whole_reply_for_an_engine_without_quick_start() {
    // Review of #235: an engine that trades the fast start for fewer
    // requests reads a reply under its limit in one synthesis.
    let whole = Arc::new(OpenEngine {
        chunk_chars: 4000,
        quick_start: false,
        ..OpenEngine::new("whole", 1)
    });
    let registry = Registry::default();
    registry.register(whole.clone()).unwrap();
    let r = Rig::config(Config::new(registry));
    r.h.set(Key::Engine, Value::Text("whole".into())).unwrap();
    r.h.speak("One. Two. Three.", QueueMode::Append, false, None)
        .unwrap();
    wait_for("one synthesis", || whole.inner.syntheses() == 1);
    assert_eq!(whole.inner.texts(), vec!["One. Two. Three."]);
}
