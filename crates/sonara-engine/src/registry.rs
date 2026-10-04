//! The engines a host has loaded, behind its licence policy (R6).
use crate::{Engine, EngineId, Error, LicenseClass, Result, Voice};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Engines by id. `register` refuses an engine whose licence class the host
/// does not allow, so a refused engine can never synthesize. Engines can
/// come and go while the registry is shared (external engine profiles are
/// added and removed at run time), hence the interior mutability.
pub struct Registry {
    allowed: Vec<LicenseClass>,
    engines: RwLock<Vec<Arc<dyn Engine>>>,
}

impl Default for Registry {
    /// Spec section 5: permissive engines and the operating system's own.
    /// External engines are opt-in for a host (`Registry::new`).
    fn default() -> Self {
        Self::new(&[LicenseClass::Permissive, LicenseClass::Os])
    }
}

impl Registry {
    /// A registry that accepts only the given licence classes.
    pub fn new(allowed: &[LicenseClass]) -> Self {
        Registry {
            allowed: allowed.to_vec(),
            engines: RwLock::new(Vec::new()),
        }
    }

    /// Only permissively licensed engines: no OS engine either.
    pub fn permissive_only() -> Self {
        Self::new(&[LicenseClass::Permissive])
    }

    pub fn allows(&self, class: LicenseClass) -> bool {
        self.allowed.contains(&class)
    }

    fn read(&self) -> RwLockReadGuard<'_, Vec<Arc<dyn Engine>>> {
        self.engines.read().unwrap_or_else(|p| p.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Vec<Arc<dyn Engine>>> {
        self.engines.write().unwrap_or_else(|p| p.into_inner())
    }

    fn check(&self, engine: &Arc<dyn Engine>) -> Result<EngineId> {
        let (id, class) = (engine.id(), engine.license_class());
        if !self.allows(class) {
            return Err(Error::LicenseRefused { engine: id, class });
        }
        Ok(id)
    }

    pub fn register(&self, engine: Arc<dyn Engine>) -> Result<()> {
        let id = self.check(&engine)?;
        let mut engines = self.write();
        if engines.iter().any(|e| e.id() == id) {
            return Err(Error::DuplicateEngine(id));
        }
        engines.push(engine);
        Ok(())
    }

    /// Remove an engine; `UnknownEngine` when it is not registered.
    pub fn unregister(&self, id: &str) -> Result<Arc<dyn Engine>> {
        let mut engines = self.write();
        let at = engines
            .iter()
            .position(|e| e.id().as_str() == id)
            .ok_or_else(|| Error::UnknownEngine(id.to_string()))?;
        Ok(engines.remove(at))
    }

    /// Put `engine` in the place of the registered engine with its id, in
    /// one step (there is no moment without it).
    pub fn replace(&self, engine: Arc<dyn Engine>) -> Result<()> {
        let id = self.check(&engine)?;
        let mut engines = self.write();
        let slot = engines
            .iter_mut()
            .find(|e| e.id() == id)
            .ok_or_else(|| Error::UnknownEngine(id.as_str().to_string()))?;
        *slot = engine;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn Engine>> {
        self.read()
            .iter()
            .find(|e| e.id().as_str() == id)
            .cloned()
            .ok_or_else(|| Error::UnknownEngine(id.to_string()))
    }

    /// Registered engine ids, in registration order.
    pub fn ids(&self) -> Vec<EngineId> {
        self.read().iter().map(|e| e.id()).collect()
    }

    /// Every voice of every registered engine (each engine's cached list,
    /// never a network call). The lock is not held while engines answer.
    pub fn voices(&self) -> Vec<Voice> {
        let engines: Vec<Arc<dyn Engine>> = self.read().clone();
        engines.iter().flat_map(|e| e.voices()).collect()
    }
}
