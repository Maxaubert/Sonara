//! `set`/`get`, `voices` and the config checks of `ReaderHandle::new`.
mod common;

use common::engines::OtherEngine;
use common::{len, play, Rig};
use sonara_audio::{OutputCall, TestOutput};
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config, Error, Key, QueueMode, ReaderHandle, Registry, Value};
use std::sync::Arc;

fn bad(r: Result<(), Error>, key: Key) {
    match r {
        Err(Error::BadValue { key: k, .. }) => assert_eq!(k, key),
        other => panic!("expected BadValue for {key}, got {other:?}"),
    }
}

#[test]
fn defaults_and_keys() {
    let (r, _) = Rig::new();
    assert_eq!(r.h.get(Key::Volume).unwrap(), Value::Number(100));
    assert_eq!(r.h.get(Key::Rate).unwrap(), Value::Number(200));
    assert_eq!(r.h.get(Key::Voice).unwrap(), Value::Null);
    assert_eq!(r.h.get(Key::Engine).unwrap(), Value::Text("fake".into()));
    for k in [Key::Volume, Key::Rate, Key::Voice, Key::Engine] {
        assert_eq!(Key::parse(k.as_str()), Some(k));
    }
    assert_eq!(Key::parse("pitch"), None);
}

#[test]
fn out_of_range_and_wrong_types_are_refused_and_change_nothing() {
    let (r, _) = Rig::new();
    bad(r.h.set(Key::Volume, Value::Number(101)), Key::Volume);
    bad(r.h.set(Key::Volume, Value::Text("50".into())), Key::Volume);
    bad(r.h.set(Key::Rate, Value::Number(99)), Key::Rate);
    bad(r.h.set(Key::Rate, Value::Number(401)), Key::Rate);
    bad(r.h.set(Key::Rate, Value::Number(u64::MAX)), Key::Rate);
    // The message echoes what was sent, not a narrowed number.
    match r.h.set(Key::Rate, Value::Number(1 << 40)) {
        Err(Error::BadValue { reason, .. }) => {
            assert!(reason.starts_with("1099511627776 "), "{reason}")
        }
        other => panic!("expected BadValue, got {other:?}"),
    }
    bad(r.h.set(Key::Voice, Value::Number(1)), Key::Voice);
    bad(r.h.set(Key::Engine, Value::Null), Key::Engine);
    assert_eq!(
        r.h.set(Key::Voice, Value::Text("nobody".into())),
        Err(Error::Engine(sonara_engine::Error::UnknownVoice(
            "nobody".into()
        )))
    );
    assert_eq!(
        r.h.set(Key::Engine, Value::Text("nope".into())),
        Err(Error::Engine(sonara_engine::Error::UnknownEngine(
            "nope".into()
        )))
    );
    assert_eq!(r.h.get(Key::Volume).unwrap(), Value::Number(100));
    assert_eq!(r.h.get(Key::Rate).unwrap(), Value::Number(200));
    assert_eq!(r.events_now(), Vec::<String>::new());
    assert_eq!(r.calls_now(), []);
}

#[test]
fn a_voice_is_set_by_id_or_name_and_reaches_the_engine() {
    let (r, _) = Rig::new();
    r.h.set(Key::Voice, Value::Text("Fake silence".into()))
        .unwrap();
    assert_eq!(r.h.get(Key::Voice).unwrap(), Value::Text("silence".into()));
    assert_eq!(r.h.state().unwrap().voice.as_deref(), Some("silence"));
    r.h.set(Key::Voice, Value::Text(String::new())).unwrap();
    assert_eq!(r.h.get(Key::Voice).unwrap(), Value::Null);
    r.h.set(Key::Voice, Value::Text("tone".into())).unwrap();
    r.h.set(Key::Voice, Value::Null).unwrap();
    assert_eq!(r.h.get(Key::Voice).unwrap(), Value::Null);
}

#[test]
fn switching_engines_resets_a_voice_the_new_engine_lacks() {
    let fake = Arc::new(FakeEngine::new());
    let other = Arc::new(OtherEngine::default());
    let registry = Registry::default();
    registry.register(fake.clone()).unwrap();
    registry.register(other.clone()).unwrap();
    let r = Rig::config(Config::new(registry));

    r.h.set(Key::Voice, Value::Text("tone".into())).unwrap();
    r.h.set(Key::Engine, Value::Text("other".into())).unwrap();
    // `other` offers `tone` too: the voice stays.
    assert_eq!(r.h.get(Key::Voice).unwrap(), Value::Text("tone".into()));
    r.h.set(Key::Engine, Value::Text("fake".into())).unwrap();
    r.h.set(Key::Voice, Value::Text("silence".into())).unwrap();
    r.h.set(Key::Engine, Value::Text("other".into())).unwrap();
    assert_eq!(r.h.get(Key::Engine).unwrap(), Value::Text("other".into()));
    assert_eq!(r.h.get(Key::Voice).unwrap(), Value::Null);

    r.h.speak("On the other engine.", QueueMode::Append, false, None)
        .unwrap();
    assert_eq!(
        r.calls(1),
        [play(1, 0, 1, len("On the other engine.", 200))]
    );
    assert_eq!(other.inner.syntheses(), 1);
    assert_eq!(fake.syntheses(), 0);
}

#[test]
fn voices_per_engine_and_for_all() {
    let registry = Registry::default();
    registry.register(Arc::new(FakeEngine::new())).unwrap();
    registry.register(Arc::new(OtherEngine::default())).unwrap();
    let r = Rig::config(Config::new(registry));
    assert_eq!(r.h.voices(None).unwrap().len(), 3);
    let ids: Vec<_> =
        r.h.voices(Some("fake"))
            .unwrap()
            .into_iter()
            .map(|v| v.id)
            .collect();
    assert_eq!(ids, ["tone", "silence"]);
    assert_eq!(
        r.h.voices(Some("nope")),
        Err(Error::Engine(sonara_engine::Error::UnknownEngine(
            "nope".into()
        )))
    );
}

fn start(config: Config) -> Result<ReaderHandle, Error> {
    let (out, rx) = TestOutput::new();
    ReaderHandle::new(config.with_output(Box::new(out), rx))
}

fn fake_registry() -> Registry {
    let registry = Registry::default();
    registry.register(Arc::new(FakeEngine::new())).unwrap();
    registry
}

#[test]
fn the_config_is_checked() {
    assert_eq!(
        start(Config::new(Registry::default())).err(),
        Some(Error::NoEngine)
    );
    let mut c = Config::new(fake_registry());
    c.engine = Some("onecore-not-here".into());
    assert!(matches!(
        start(c).err(),
        Some(Error::Engine(sonara_engine::Error::UnknownEngine(_)))
    ));
    let mut c = Config::new(fake_registry());
    c.voice = Some("nobody".into());
    assert!(matches!(
        start(c).err(),
        Some(Error::Engine(sonara_engine::Error::UnknownVoice(_)))
    ));
    let mut c = Config::new(fake_registry());
    c.rate = 50;
    assert!(matches!(
        start(c).err(),
        Some(Error::BadValue { key: Key::Rate, .. })
    ));
    let mut c = Config::new(fake_registry());
    c.volume = 101;
    assert!(matches!(
        start(c).err(),
        Some(Error::BadValue {
            key: Key::Volume,
            ..
        })
    ));
}

#[test]
fn the_config_settings_apply_from_the_start() {
    let mut c = Config::new(fake_registry());
    c.voice = Some("Fake silence".into());
    c.rate = 300;
    c.volume = 40;
    let r = Rig::config(c);
    assert_eq!(r.h.get(Key::Voice).unwrap(), Value::Text("silence".into()));
    assert_eq!(r.h.get(Key::Rate).unwrap(), Value::Number(300));
    assert_eq!(r.h.get(Key::Volume).unwrap(), Value::Number(40));
    assert_eq!(r.out.volume(), 40);
    assert_eq!(r.calls_now(), [OutputCall::SetVolume(40)]);
}
