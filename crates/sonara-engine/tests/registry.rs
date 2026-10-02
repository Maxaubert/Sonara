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
    let mut reg = Registry::permissive_only();
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
    let mut reg = Registry::new(&[]);
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
    let mut reg = Registry::default();
    reg.register(Arc::new(FakeEngine::new())).unwrap();
    reg.register(Arc::new(OsEngine)).unwrap();
    assert_eq!(reg.ids(), vec![EngineId("fake"), EngineId("os-test")]);
    assert_eq!(reg.get("os-test").unwrap().id(), EngineId("os-test"));
}

#[test]
fn registering_an_id_twice_is_refused() {
    let mut reg = Registry::default();
    reg.register(Arc::new(FakeEngine::new())).unwrap();
    assert_eq!(
        reg.register(Arc::new(FakeEngine::new())),
        Err(Error::DuplicateEngine(EngineId("fake")))
    );
}

#[test]
fn registry_voices_cover_every_engine() {
    let mut reg = Registry::default();
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
