//! The licence policy (R6): a registry only holds engines its host allows.
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, EngineId, Error, LicenseClass, PcmStream, Registry, Result, Voice};
use std::sync::Arc;

/// An engine that claims the OS licence class, like OneCore, but runs anywhere.
struct OsEngine;

impl Engine for OsEngine {
    fn id(&self) -> EngineId {
        EngineId("os-test")
    }
    fn license_class(&self) -> LicenseClass {
        LicenseClass::Os
    }
    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }
    fn warm(&self) -> Result<()> {
        Ok(())
    }
    fn synthesize(&self, _: &str, _: &str, _: u32) -> Result<PcmStream> {
        Ok(Box::new(std::iter::empty()))
    }
    fn cancel(&self) {}
}

#[test]
fn permissive_only_registry_refuses_non_permissive_engines() {
    let reg = Registry::permissive_only();
    assert_eq!(
        reg.register(Arc::new(OsEngine)),
        Err(Error::LicenseRefused {
            engine: EngineId("os-test"),
            class: LicenseClass::Os,
        })
    );
    assert!(reg.ids().is_empty());
    assert!(matches!(
        reg.get("os-test"),
        Err(Error::UnknownEngine(id)) if id == "os-test"
    ));
    reg.register(Arc::new(FakeEngine::new())).unwrap();
    assert_eq!(reg.ids(), vec![EngineId("fake")]);
}

#[test]
fn a_registry_that_allows_nothing_refuses_every_engine() {
    let reg = Registry::new(&[]);
    assert!(matches!(
        reg.register(Arc::new(FakeEngine::new())),
        Err(Error::LicenseRefused {
            class: LicenseClass::Permissive,
            ..
        })
    ));
}

#[test]
fn default_registry_allows_permissive_and_os_engines() {
    let reg = Registry::default();
    reg.register(Arc::new(FakeEngine::new())).unwrap();
    reg.register(Arc::new(OsEngine)).unwrap();
    assert_eq!(reg.ids(), vec![EngineId("fake"), EngineId("os-test")]);
    assert_eq!(reg.get("os-test").unwrap().id(), EngineId("os-test"));
}

#[test]
fn registering_an_id_twice_is_refused() {
    let reg = Registry::default();
    reg.register(Arc::new(FakeEngine::new())).unwrap();
    assert_eq!(
        reg.register(Arc::new(FakeEngine::new())),
        Err(Error::DuplicateEngine(EngineId("fake")))
    );
}

#[test]
fn registry_voices_cover_every_engine() {
    let reg = Registry::default();
    reg.register(Arc::new(FakeEngine::new())).unwrap();
    let voices = reg.voices();
    assert_eq!(
        voices.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        vec!["tone", "silence"]
    );
    assert!(voices
        .iter()
        .all(|v| v.engine == EngineId("fake") && v.license_class == LicenseClass::Permissive));
}

#[test]
fn refusal_message_names_the_engine_and_class() {
    let e = Error::LicenseRefused {
        engine: EngineId("onecore"),
        class: LicenseClass::Os,
    };
    assert_eq!(
        e.to_string(),
        "engine 'onecore' refused: licence class Os is not allowed by this host"
    );
}

#[test]
fn engine_status_reads_well_in_a_log() {
    use sonara_engine::{EngineId, EngineStatus, Readiness};
    let s = EngineStatus {
        readiness: Readiness::Downloading,
        progress: Some((40, 100)),
        fallback: Some(EngineId("onecore")),
        message: None,
        reason: None,
    };
    assert_eq!(
        s.to_string(),
        "downloading its model (40%); speaking with onecore meanwhile"
    );
    let s = EngineStatus {
        readiness: Readiness::Waiting,
        progress: None,
        fallback: None,
        message: Some("HTTP 503".into()),
        reason: None,
    };
    assert_eq!(s.to_string(), "not ready, retrying later: HTTP 503");
    assert_eq!(EngineStatus::ready().to_string(), "ready");
}

/// An engine of the `External` class (a user's profile), with any id.
struct ExternalEngine(&'static str, usize);

impl Engine for ExternalEngine {
    fn id(&self) -> EngineId {
        EngineId::intern(self.0)
    }
    fn license_class(&self) -> LicenseClass {
        LicenseClass::External
    }
    fn voices(&self) -> Vec<Voice> {
        Vec::new()
    }
    fn warm(&self) -> Result<()> {
        Ok(())
    }
    fn synthesize(&self, _: &str, _: &str, _: u32) -> Result<PcmStream> {
        Ok(Box::new(std::iter::empty()))
    }
    fn cancel(&self) {}
    fn lookahead(&self) -> usize {
        self.1
    }
}

#[test]
fn intern_returns_the_same_static_str() {
    let a = EngineId::intern(&String::from("my-openai"));
    let b = EngineId::intern(&format!("my-{}", "openai"));
    assert_eq!(a, b);
    assert!(std::ptr::eq(a.as_str(), b.as_str()), "one leak per id");
    assert_eq!(a.as_str(), "my-openai");
    assert_ne!(EngineId::intern("other"), a);
}

#[test]
fn external_class_is_refused_by_the_default_registry() {
    let reg = Registry::default();
    assert!(matches!(
        reg.register(Arc::new(ExternalEngine("cloud", 2))),
        Err(Error::LicenseRefused {
            class: LicenseClass::External,
            ..
        })
    ));
    let reg = Registry::new(&[
        LicenseClass::Permissive,
        LicenseClass::Os,
        LicenseClass::External,
    ]);
    reg.register(Arc::new(ExternalEngine("cloud", 2))).unwrap();
    assert_eq!(reg.get("cloud").unwrap().lookahead(), 2);
}

fn open_registry() -> Registry {
    Registry::new(&[LicenseClass::Permissive, LicenseClass::External])
}

#[test]
fn register_and_unregister_while_shared() {
    let reg = Arc::new(open_registry());
    reg.register(Arc::new(FakeEngine::new())).unwrap();
    let other = reg.clone();
    std::thread::spawn(move || other.register(Arc::new(ExternalEngine("p1", 1))).unwrap())
        .join()
        .unwrap();
    assert_eq!(reg.ids(), vec![EngineId("fake"), EngineId::intern("p1")]);
    let gone = reg.unregister("p1").unwrap();
    assert_eq!(gone.id(), EngineId::intern("p1"));
    assert_eq!(reg.ids(), vec![EngineId("fake")]);
}

#[test]
fn replace_keeps_one_entry() {
    let reg = open_registry();
    reg.register(Arc::new(FakeEngine::new())).unwrap();
    reg.register(Arc::new(ExternalEngine("p2", 1))).unwrap();
    reg.replace(Arc::new(ExternalEngine("p2", 3))).unwrap();
    assert_eq!(reg.ids(), vec![EngineId("fake"), EngineId::intern("p2")]);
    assert_eq!(reg.get("p2").unwrap().lookahead(), 3);
    assert!(matches!(
        reg.replace(Arc::new(ExternalEngine("p3", 1))),
        Err(Error::UnknownEngine(id)) if id == "p3"
    ));
    // The licence rule applies to a replacement too.
    let strict = Registry::default();
    strict.register(Arc::new(FakeEngine::new())).unwrap();
    assert!(matches!(
        strict.replace(Arc::new(ExternalEngine("fake", 1))),
        Err(Error::LicenseRefused { .. })
    ));
}

#[test]
fn unregister_unknown_is_unknown_engine() {
    let reg = open_registry();
    assert!(matches!(
        reg.unregister("nope"),
        Err(Error::UnknownEngine(id)) if id == "nope"
    ));
}
