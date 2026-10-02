//! The engines a host has loaded, behind its licence policy (R6).
use crate::{Engine, EngineId, Error, LicenseClass, Result, Voice};
use std::sync::Arc;

/// Engines by id. `register` refuses an engine whose licence class the host
/// does not allow, so a refused engine can never synthesize.
pub struct Registry {
    allowed: Vec<LicenseClass>,
    engines: Vec<Arc<dyn Engine>>,
}

impl Default for Registry {
    /// Spec section 5: permissive engines and the operating system's own.
    fn default() -> Self {
        Self::new(&[LicenseClass::Permissive, LicenseClass::Os])
    }
}

impl Registry {
    /// A registry that accepts only the given licence classes.
    pub fn new(allowed: &[LicenseClass]) -> Self {
        Registry {
            allowed: allowed.to_vec(),
            engines: Vec::new(),
        }
    }

    /// Only permissively licensed engines: no OS engine either.
    pub fn permissive_only() -> Self {
        Self::new(&[LicenseClass::Permissive])
    }

    pub fn allows(&self, class: LicenseClass) -> bool {
        self.allowed.contains(&class)
    }

    pub fn register(&mut self, engine: Arc<dyn Engine>) -> Result<()> {
        let (id, class) = (engine.id(), engine.license_class());
        if !self.allows(class) {
            return Err(Error::LicenseRefused { engine: id, class });
        }
        if self.engines.iter().any(|e| e.id() == id) {
            return Err(Error::DuplicateEngine(id));
        }
        self.engines.push(engine);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn Engine>> {
        self.engines
            .iter()
            .find(|e| e.id().as_str() == id)
            .cloned()
            .ok_or_else(|| Error::UnknownEngine(id.to_string()))
    }

    /// Registered engine ids, in registration order.
    pub fn ids(&self) -> Vec<EngineId> {
        self.engines.iter().map(|e| e.id()).collect()
    }

    /// Every voice of every registered engine.
    pub fn voices(&self) -> Vec<Voice> {
        self.engines.iter().flat_map(|e| e.voices()).collect()
    }
}
