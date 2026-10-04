//! The worker asks the engine for its prefetch depth (spec D10) and lets an
//! engine that accepts unlisted voices take any voice id; a replaced engine
//! applies on the next `set engine`.
mod common;

use common::engines::{GateEngine, OpenEngine};
use common::{Rig, THREE, TIMEOUT};
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config, Error, Key, QueueMode, Registry, SendMode, Value};
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

/// #235: an engine in send mode `message` reads an item as one text (one
/// synthesis), split only past its input limit; switching to an engine in
/// `sentence` mode reads one sentence per synthesis again. The handle's
/// `send_mode` follows the engine (L3 joins what it releases by it).
#[test]
fn an_engine_in_message_mode_gets_the_whole_item() {
    let whole = Arc::new(OpenEngine {
        send_mode: SendMode::Message,
        ..OpenEngine::new("whole", 1)
    });
    let plain = Arc::new(OpenEngine::new("plain", 1));
    let registry = Registry::default();
    registry.register(plain.clone()).unwrap();
    registry.register(whole.clone()).unwrap();
    let r = Rig::config(Config::new(registry));
    assert_eq!(r.h.send_mode(), SendMode::Sentence);
    r.h.set(Key::Engine, Value::Text("whole".into())).unwrap();
    assert_eq!(r.h.send_mode(), SendMode::Message);
    r.h.speak("One. Two.\n\nThree.", QueueMode::Append, false, None)
        .unwrap();
    wait_for("one synthesis", || whole.inner.syntheses() == 1);
    assert_eq!(whole.inner.texts(), vec!["One. Two.\n\nThree."]);
    r.h.set(Key::Engine, Value::Text("plain".into())).unwrap();
    assert_eq!(r.h.send_mode(), SendMode::Sentence);
    r.h.speak("Four. Five.", QueueMode::Replace, true, None)
        .unwrap();
    wait_for("one sentence each", || plain.inner.syntheses() == 2);
    assert_eq!(plain.inner.texts(), vec!["Four.", "Five."]);
}

#[test]
fn a_message_past_the_engines_limit_is_split_at_paragraphs() {
    let whole = Arc::new(OpenEngine {
        send_mode: SendMode::Message,
        limit: 30,
        ..OpenEngine::new("whole", 2)
    });
    let r = Rig::with(whole.clone());
    r.h.speak(
        "The first one here.\n\nThe second one here.",
        QueueMode::Append,
        false,
        None,
    )
    .unwrap();
    wait_for("two syntheses", || whole.inner.syntheses() == 2);
    assert_eq!(
        whole.inner.texts(),
        vec!["The first one here.", "The second one here."]
    );
}
