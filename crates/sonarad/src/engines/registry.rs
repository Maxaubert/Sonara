//! The registered engines: building, adding, removing and keying profiles.
use super::*;

/// The reply of `engine_models` (#235).
pub struct Models {
    pub models: Vec<ModelInfo>,
    /// The provider has a model list API (else the model is typed in).
    pub list: bool,
    /// The kind takes a model at all.
    pub takes_model: bool,
    pub required: bool,
    pub error: Option<sonara_engine::Error>,
}

impl Models {
    /// The models of `ext`, fetched first when `fetch` (never while muted:
    /// `External::refresh_models` keeps the known list then).
    pub(super) fn of(ext: &External, fetch: bool) -> Models {
        let list = ext.has_model_list();
        let (models, error) = if fetch && list {
            match ext.refresh_models() {
                Ok(m) => (m, None),
                Err(e) => (ext.models(), Some(e)),
            }
        } else {
            (ext.models(), None)
        };
        Models {
            models,
            list,
            takes_model: ext.profile().takes_model(),
            required: ext.profile().model_required(),
            error,
        }
    }
}

impl Engines {
    pub(super) fn build(&self, raw: &Value) -> Result<Arc<External>, ProfileError> {
        let mut profile = Profile::from_json(raw)?;
        if matches!(profile.key_ref, KeyRef::Env(_)) {
            profile.key_origin = raw
                .get(KEY_ORIGIN)
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        let mut config = ExternalConfig::new(profile, self.keys.clone());
        config.hold = Some(self.hold.clone());
        config.fallback = self.fallback.clone();
        config.fallback_voice = self.fallback_voice.clone();
        let (log, gate) = (self.log.clone(), self.gate.clone());
        config.notice = Some(Arc::new(move |n: Notice| {
            let key = (n.engine.as_str().to_string(), n.reason);
            {
                let mut g = gate.lock().unwrap_or_else(|p| p.into_inner());
                let now = Instant::now();
                if g.get(&key)
                    .is_some_and(|t| now.duration_since(*t) < NOTICE_EVERY)
                {
                    return;
                }
                g.insert(key, now);
            }
            sonara_system::log::emit(log.as_ref(), &notice_line(&n));
        }));
        Ok(Arc::new(External::new(config)?))
    }

    /// Register every usable profile in `registry` and keep it in step
    /// with later changes (the reader's, the previews').
    pub fn attach(&self, registry: Arc<Registry>) {
        for e in self.lock().iter() {
            if let State::Ready(x) = &e.state {
                let _ = registry.register(x.clone());
            }
        }
        self.registries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(registry);
    }

    pub(super) fn registries(&self) -> Vec<Arc<Registry>> {
        self.registries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The engine of a usable profile.
    pub fn get(&self, id: &str) -> Option<Arc<External>> {
        self.lock().iter().find_map(|e| match &e.state {
            State::Ready(x) if e.id == id => Some(x.clone()),
            _ => None,
        })
    }

    pub fn contains(&self, id: &str) -> bool {
        self.lock().iter().any(|e| e.id == id)
    }

    /// Before a save: take in what was written to the file since it was
    /// last read or saved (a lost update otherwise). A file that is not
    /// JSON is refused, never overwritten.
    /// Delete the stored key of `raw`'s id when it is bound to another
    /// origin than the entry's (or to none).
    pub(super) fn drop_key_bound_elsewhere(&self, raw: &Value) {
        let Some(id) = raw.get("id").and_then(Value::as_str) else {
            return;
        };
        let store = self.keys.store();
        if let Ok(Some(k)) = store.get(id) {
            if k.origin != origin_of(raw) {
                match store.delete(id) {
                    Ok(()) => self.note(&format!(
                        "engine {id}: its key was for another address; deleted"
                    )),
                    Err(e) => self.note(&format!("engine {id}: the key could not be deleted: {e}")),
                }
            }
        }
    }

    pub(super) fn check_secret(secret: &str) -> Result<(), Failure> {
        if secret.trim().is_empty() {
            return Err(bad("'secret' is empty"));
        }
        if secret.len() > MAX_KEY_BYTES {
            return Err(bad("key too long"));
        }
        Ok(())
    }

    /// `engine_add`: validate, store the secret, save, register. Returns
    /// whether it replaced an existing profile.
    pub fn add(
        &self,
        engine: &Value,
        secret: Option<&str>,
        replace: bool,
    ) -> Result<bool, Failure> {
        let mut raw = match engine {
            Value::Object(m) => m.clone(),
            _ => return Err(bad("'engine' must be an object with the profile")),
        };
        // First, before any other check: a program is never added over the
        // protocol, and no reply says whether its path exists.
        if is_command(engine) {
            return Err(Failure::new(Code::Forbidden, COMMAND_FORBIDDEN));
        }
        if let Some(id) = engine.get("id").and_then(Value::as_str) {
            if self.lock().iter().any(|e| e.id == id && is_command(&e.raw)) {
                return Err(Failure::new(Code::Forbidden, COMMAND_FORBIDDEN));
            }
        }
        if let Some(s) = secret {
            Self::check_secret(s)?;
            match raw.get("key_ref") {
                None | Some(Value::Null) => {
                    raw.insert("key_ref".into(), json!("credman"));
                }
                Some(Value::String(k)) if k == "credman" => {}
                Some(Value::String(k)) if k.starts_with("env:") => {
                    return Err(bad(format!(
                        "this engine reads its key from the environment variable {}; \
                         do not send a secret",
                        &k[4..]
                    )))
                }
                Some(_) => return Err(bad("this engine has key_ref none: it takes no key")),
            }
        }
        let profile = Profile::from_json(&Value::Object(raw))
            .and_then(|p| {
                // A new profile's program must exist now (a stored one whose
                // program is gone stays and reads with the fallback).
                p.check_new()?;
                Ok(p)
            })
            .map_err(profile_failure)?;
        if profile.kind == Kind::Command {
            return Err(Failure::new(Code::Forbidden, COMMAND_FORBIDDEN));
        }
        let mut stored = profile.to_json();
        let id = profile.id.clone();
        let origin = profile.origin();
        self.take_in_file_changes()?;
        let mut entries = self.lock();
        let at = entries.iter().position(|e| e.id == id);
        // Checked again under the lock that saves (a reload in between).
        if at.is_some_and(|i| is_command(&entries[i].raw)) {
            return Err(Failure::new(Code::Forbidden, COMMAND_FORBIDDEN));
        }
        match at {
            Some(_) if !replace => {
                return Err(bad(format!("engine '{id}' exists; send replace: true")))
            }
            None if entries.len() >= MAX_PROFILES => {
                return Err(bad(format!("at most {MAX_PROFILES} engines")))
            }
            _ => {}
        }
        // An env key's confirmed origin survives a replace only when the
        // variable and the origin stay the same: a protocol client can
        // never confirm a new one.
        if let KeyRef::Env(_) = &profile.key_ref {
            let old = at.map(|i| &entries[i].raw);
            let confirmed = old
                .filter(|r| r.get("key_ref") == stored.get("key_ref"))
                .and_then(|r| r.get(KEY_ORIGIN))
                .and_then(Value::as_str)
                .filter(|o| Some(*o) == origin.as_deref());
            if let (Some(c), Some(m)) = (confirmed, stored.as_object_mut()) {
                m.insert(KEY_ORIGIN.into(), json!(c));
            }
        }
        let store = self.keys.store();
        match secret {
            Some(s) => {
                let o = origin
                    .as_deref()
                    .ok_or_else(|| bad("this engine has no address to bind a key to"))?;
                store.set(&id, &Secret::new(s), o).map_err(|e| {
                    Failure::new(Code::Engine, format!("cannot store the key: {e}"))
                })?;
            }
            None => {
                // A key entered for another origin (or none) is never sent
                // here: delete it rather than keep a key that a later
                // change could point somewhere else.
                if let Ok(Some(k)) = store.get(&id) {
                    if k.origin != origin {
                        match store.delete(&id) {
                            Ok(()) => self.note(&format!(
                                "engine {id}: its key was for another address; deleted"
                            )),
                            Err(e) => self
                                .note(&format!("engine {id}: the key could not be deleted: {e}")),
                        }
                    }
                }
            }
        }
        let ext = self.build(&stored).map_err(|e| bad(e.to_string()))?;
        let was_registered = at.is_some_and(|i| matches!(entries[i].state, State::Ready(_)));
        let new_entry = || Entry {
            id: id.clone(),
            raw: stored.clone(),
            state: State::Ready(ext.clone()),
        };
        let mut next: Vec<Entry> = entries
            .drain(..)
            .map(|e| if e.id == id { new_entry() } else { e })
            .collect();
        if at.is_none() {
            next.push(new_entry());
        }
        let saved = self.save(&next);
        *entries = next;
        saved?;
        drop(entries);
        for r in self.registries() {
            let done = if was_registered {
                r.replace(ext.clone())
            } else {
                r.register(ext.clone())
            };
            if let Err(e) = done {
                self.note(&format!("engine {id}: not registered: {e}"));
            }
        }
        self.note(&format!(
            "engine add id={id} kind={} host={}{}",
            ext.profile().kind.as_str(),
            ext.profile().sends_text_to(),
            if secret.is_some() { " key=set" } else { "" }
        ));
        Ok(at.is_some())
    }

    /// The engine of a profile that is not saved, for its voice and model
    /// lists (protocol 1.4, #227; 1.5, #235): built for this request only,
    /// under `DRAFT_ID`, with `secret` as its only stored key (never
    /// another profile's): nothing is saved, stored or registered. Its id,
    /// model and voice may be missing. A `command` profile is
    /// `E_FORBIDDEN`, as in `engine_add`: a request never runs a program it
    /// names.
    pub(super) fn draft(&self, profile: &Value, secret: Option<&str>) -> Result<External, Failure> {
        let mut raw = match profile {
            Value::Object(m) => m.clone(),
            _ => return Err(bad("'profile' must be an object with the profile")),
        };
        if is_command(profile) {
            return Err(Failure::new(Code::Forbidden, COMMAND_FORBIDDEN));
        }
        raw.insert("id".into(), json!(DRAFT_ID));
        raw.remove(KEY_ORIGIN);
        if let Some(s) = secret {
            Self::check_secret(s)?;
            match raw.get("key_ref") {
                None | Some(Value::Null) => {
                    raw.insert("key_ref".into(), json!("credman"));
                }
                Some(Value::String(k)) if k == "credman" => {}
                Some(_) => return Err(bad("send a secret only with key_ref credman")),
            }
        }
        let profile = Profile::from_json(&Value::Object(raw)).map_err(profile_failure)?;
        if profile.kind == Kind::Command {
            return Err(Failure::new(Code::Forbidden, COMMAND_FORBIDDEN));
        }
        let store = Arc::new(MemoryStore::new());
        if let Some(s) = secret {
            let o = profile.origin().ok_or_else(|| {
                bad("this engine has no address yet (a region or url) to send the key to")
            })?;
            store
                .set(DRAFT_ID, &Secret::new(s), &o)
                .map_err(|e| bad(format!("cannot use the key: {e}")))?;
        }
        let mut config = ExternalConfig::new(profile, KeyResolver::new(store));
        // Muted: its lists are not fetched either.
        config.hold = Some(self.hold.clone());
        External::new(config).map_err(profile_failure)
    }

    /// `voices` with `profile` (protocol 1.4, #227): the voice list of a
    /// profile that is not saved, so the settings page can offer voices
    /// before the user picks one (`draft`). Returns the voices and the
    /// error of the fetch, when it failed.
    pub fn draft_voices(
        &self,
        profile: &Value,
        secret: Option<&str>,
    ) -> Result<(Vec<Voice>, Option<sonara_engine::Error>), Failure> {
        let ext = self.draft(profile, secret)?;
        Ok(match ext.refresh_voices() {
            Ok(v) => (v, None),
            Err(e) => (ext.voices(), Some(e)),
        })
    }

    /// `engine_models` with `profile` (protocol 1.5, #235): the provider's
    /// models for a profile that is not saved (`draft`), whether it has a
    /// list at all, and the error of the fetch, when it failed.
    pub fn draft_models(&self, profile: &Value, secret: Option<&str>) -> Result<Models, Failure> {
        let ext = self.draft(profile, secret)?;
        Ok(Models::of(&ext, true))
    }

    /// `engine_models` with `engine`: a saved profile's models, fetched
    /// again when the cache is old or `refresh` is set.
    pub fn models(&self, id: &str, refresh: bool) -> Result<Models, Failure> {
        let ext = match self.get(id) {
            Some(x) => x,
            None if self.contains(id) => {
                return Err(Failure::new(
                    Code::Unsupported,
                    format!("engine '{id}' cannot be used by this version"),
                ))
            }
            None => return Err(Failure::new(Code::NotFound, format!("no engine '{id}'"))),
        };
        Ok(Models::of(&ext, refresh || ext.models_stale()))
    }

    /// `engine_remove` (after the caller moved off it when current).
    pub fn remove(&self, id: &str, forget_key: bool) -> Result<(), Failure> {
        self.take_in_file_changes()?;
        let mut entries = self.lock();
        let at = entries
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| Failure::new(Code::NotFound, format!("no engine '{id}'")))?;
        let removed = entries.remove(at);
        let saved = self.save(&entries);
        drop(entries);
        if let State::Ready(x) = &removed.state {
            x.cancel();
            for r in self.registries() {
                let _ = r.unregister(id);
            }
        }
        if forget_key {
            if let Err(e) = self.keys.store().delete(id) {
                self.note(&format!("engine {id}: the key could not be deleted: {e}"));
            }
        }
        self.note(&format!("engine remove id={id}"));
        saved
    }

    /// `engine_key`: store (or with `None` delete) a profile's key.
    /// Returns whether a key resolves now.
    pub fn set_key(&self, id: &str, secret: Option<&str>) -> Result<bool, Failure> {
        let (key_ref, ext, origin) = {
            let entries = self.lock();
            let e = entries
                .iter()
                .find(|e| e.id == id)
                .ok_or_else(|| Failure::new(Code::NotFound, format!("no engine '{id}'")))?;
            match &e.state {
                State::Ready(x) => (
                    x.profile().key_ref.clone(),
                    Some(x.clone()),
                    x.profile().origin(),
                ),
                _ => (
                    KeyRef::parse(e.raw.get("key_ref"))
                        .ok()
                        .flatten()
                        .unwrap_or(KeyRef::CredMan),
                    None,
                    origin_of(&e.raw),
                ),
            }
        };
        match &key_ref {
            KeyRef::Env(name) => {
                return Err(bad(format!(
                    "engine '{id}' reads its key from the environment variable {name}"
                )))
            }
            KeyRef::None => return Err(bad(format!("engine '{id}' takes no key (key_ref none)"))),
            KeyRef::CredMan => {}
        }
        let store = self.keys.store();
        match secret {
            Some(s) => {
                Self::check_secret(s)?;
                let o = origin
                    .as_deref()
                    .ok_or_else(|| bad(format!("engine '{id}' has no address to bind a key to")))?;
                store.set(id, &Secret::new(s), o)
            }
            None => store.delete(id),
        }
        .map_err(|e| Failure::new(Code::Engine, format!("cannot store the key: {e}")))?;
        self.note(&format!(
            "engine key id={id} {}",
            if secret.is_some() { "set" } else { "cleared" }
        ));
        Ok(match ext {
            Some(x) => {
                x.key_changed();
                x.key_present()
            }
            None => secret.is_some(),
        })
    }

    /// Profile ids, in the file's order.
    pub fn ids(&self) -> Vec<String> {
        self.lock().iter().map(|e| e.id.clone()).collect()
    }
}
