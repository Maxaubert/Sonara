//! External engine profiles in this runtime (spec docs/plans/
//! 2026-10-04-external-engines-spec.md, sections 5 to 8): `engines.json` in
//! the home, one `External` engine per supported profile, registered in the
//! reader's and the previews' registries, keys in a `KeyStore`, and the
//! fallback notices as `sonarad.log` lines.
//!
//! - `engines.json` is read once at start, before the reader starts, so a
//!   saved `engine` naming a profile works on the first sentence. A file
//!   that is not JSON is copied to `engines.json.bad` and treated as empty
//!   until the next save; an entry that fails validation, or of a kind this
//!   build lacks, is kept in the file, listed, and not registered.
//! - Never a key in the file, a log line or a reply: secrets go only to the
//!   `KeyStore` (Credential Manager, or `fake-keys.json` with `--keys fake`).
use crate::wire::{self, Code, Failure};
use serde_json::{json, Map, Value};
use sonara_engine::external::keys::{KeyResolver, KeyStore, Secret, MAX_KEY_BYTES};
use sonara_engine::external::profile::{
    implemented_kinds, KeyRef, Preset, Profile, ProfileError, MAX_PROFILES,
};
use sonara_engine::external::{External, ExternalConfig, Notice};
use sonara_engine::{Engine, Reason, Registry};
use sonara_system::LogFn;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const FILE: &str = "engines.json";
/// One fallback line per (engine, reason) per this long (spec 8.3).
pub const NOTICE_EVERY: Duration = Duration::from_secs(60);

/// How to build the engines of this runtime.
pub struct Setup {
    pub home: PathBuf,
    pub store: Arc<dyn KeyStore>,
    /// What speaks when a profile cannot (Kokoro, or the fake engine).
    pub fallback: Option<Arc<dyn Engine>>,
    pub fallback_voice: String,
    /// The engine `engine_remove` switches to when it removes the current
    /// one (Kokoro when installed, else OneCore; `fake` in test runs).
    pub default_engine: String,
    pub log: Option<LogFn>,
}

enum State {
    Ready(Arc<External>),
    /// A kind this build lacks.
    Unsupported,
    /// Fails validation (the reason).
    Invalid(String),
}

struct Entry {
    id: String,
    /// As stored (an unsupported or invalid entry is kept as it was).
    raw: Value,
    state: State,
}

/// Time-limited notice lines, by (engine, reason).
type Gate = Mutex<HashMap<(String, Option<Reason>), Instant>>;

pub struct Engines {
    file: PathBuf,
    keys: KeyResolver,
    fallback: Option<Arc<dyn Engine>>,
    fallback_voice: String,
    default_engine: String,
    log: Option<LogFn>,
    gate: Arc<Gate>,
    registries: Mutex<Vec<Arc<Registry>>>,
    entries: Mutex<Vec<Entry>>,
}

fn bad(m: impl Into<String>) -> Failure {
    Failure::new(Code::BadRequest, m)
}

/// The notice line of spec 8.3, or `None` while the gate holds it back.
pub fn notice_line(n: &Notice) -> String {
    match n.reason {
        None => format!("engine {} recovered", n.engine),
        Some(r) => format!(
            "engine {} fallback reason={}{} -> {}: {}",
            n.engine,
            r.as_str(),
            n.status.map(|s| format!(" status={s}")).unwrap_or_default(),
            n.fallback.map(|f| f.as_str()).unwrap_or("none"),
            sonara_log::mask(&n.message)
        ),
    }
}

impl Engines {
    /// Read `engines.json` and build the engines; the problems found are
    /// for the log.
    pub fn load(setup: Setup) -> (Arc<Engines>, Vec<String>) {
        let file = setup.home.join(FILE);
        let mut problems = Vec::new();
        let raws: Vec<Value> = match std::fs::read_to_string(&file) {
            Err(_) => Vec::new(),
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(v) => v
                    .get("engines")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                Err(e) => {
                    let bad = setup.home.join(format!("{FILE}.bad"));
                    let _ = std::fs::copy(&file, &bad);
                    problems.push(format!(
                        "{FILE} is not valid JSON ({e}); copied to {FILE}.bad, no external \
                         engines until the next change"
                    ));
                    Vec::new()
                }
            },
        };
        let engines = Arc::new(Engines {
            file,
            keys: KeyResolver::new(setup.store),
            fallback: setup.fallback,
            fallback_voice: setup.fallback_voice,
            default_engine: setup.default_engine,
            log: setup.log,
            gate: Arc::new(Mutex::new(HashMap::new())),
            registries: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
        });
        let mut entries = Vec::new();
        for raw in raws.into_iter().take(MAX_PROFILES) {
            let id = raw
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if id.is_empty() || entries.iter().any(|e: &Entry| e.id == id) {
                problems.push(format!("{FILE}: an entry without a unique id was skipped"));
                continue;
            }
            let state = match engines.build(&raw) {
                Ok(e) => State::Ready(e),
                Err(ProfileError::Unsupported { kind, .. }) => {
                    problems.push(format!(
                        "{FILE}: engine '{id}' of kind '{kind}' is not supported by this \
                         version; kept, not used"
                    ));
                    State::Unsupported
                }
                Err(ProfileError::Invalid(m)) => {
                    problems.push(format!("{FILE}: engine '{id}' is not used: {m}"));
                    State::Invalid(m)
                }
            };
            entries.push(Entry { id, raw, state });
        }
        *engines.lock() = entries;
        (engines, problems)
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn note(&self, line: &str) {
        sonara_system::log::emit(self.log.as_ref(), line);
    }

    fn build(&self, raw: &Value) -> Result<Arc<External>, ProfileError> {
        let profile = Profile::from_json(raw)?;
        let mut config = ExternalConfig::new(profile, self.keys.clone());
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

    fn registries(&self) -> Vec<Arc<Registry>> {
        self.registries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn default_engine(&self) -> &str {
        &self.default_engine
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

    fn save(&self, entries: &[Entry]) -> Result<(), Failure> {
        let v = json!({
            "format": 1,
            "engines": entries.iter().map(|e| e.raw.clone()).collect::<Vec<_>>(),
        });
        crate::config::write_json(&self.file, &v)
            .map_err(|e| Failure::new(Code::Engine, format!("cannot save {FILE}: {e}")))
    }

    /// The profile view of spec 10.1 (never a secret).
    fn view(&self, e: &Entry, current: &str, status: Option<Value>) -> Value {
        let mut m = match &e.state {
            State::Ready(x) => {
                let p = x.profile();
                let mut m = p.to_json().as_object().cloned().unwrap_or_default();
                // The values in force, defaults of the preset included.
                if let Some(u) = p.base_url() {
                    m.insert("url".into(), json!(u));
                }
                if let Some(v) = p.effective_model() {
                    m.insert("model".into(), json!(v));
                }
                if let Some(v) = p.effective_voice() {
                    m.insert("voice".into(), json!(v));
                }
                m.insert("label".into(), json!(p.display_label()));
                m.insert("key_present".into(), json!(x.key_present()));
                m.insert("sends_text_to".into(), json!(p.sends_text_to()));
                m.insert("local".into(), json!(p.is_local()));
                m.insert("supported".into(), json!(true));
                m
            }
            State::Unsupported | State::Invalid(_) => {
                let mut m = Map::new();
                for k in [
                    "id", "kind", "label", "url", "model", "voice", "key_ref", "options",
                ] {
                    if let Some(v) = e.raw.get(k) {
                        m.insert(k.into(), v.clone());
                    }
                }
                m.insert("key_present".into(), json!(false));
                let host = e
                    .raw
                    .get("url")
                    .and_then(Value::as_str)
                    .and_then(|u| sonara_engine::external::profile::Url::parse(u).ok());
                m.insert(
                    "sends_text_to".into(),
                    json!(host.as_ref().map(|u| u.host.clone()).unwrap_or_default()),
                );
                m.insert("local".into(), json!(host.is_some_and(|u| u.is_loopback())));
                m.insert(
                    "supported".into(),
                    json!(matches!(e.state, State::Invalid(_))),
                );
                if let State::Invalid(why) = &e.state {
                    m.insert("error".into(), json!(why));
                }
                m
            }
        };
        m.insert("license_class".into(), json!("external"));
        m.insert("current".into(), json!(e.id == current));
        m.insert("status".into(), status.unwrap_or(Value::Null));
        Value::Object(m)
    }

    fn status_of(e: &Entry) -> Option<Value> {
        match &e.state {
            State::Ready(x) => {
                let mut s = wire::engine_status_json(&e.id, &x.status());
                s.as_object_mut().map(|o| o.remove("engine"));
                Some(s)
            }
            _ => None,
        }
    }

    /// `engine_list`.
    pub fn list(&self, current: &str) -> Map<String, Value> {
        let entries = self.lock();
        let views: Vec<Value> = entries
            .iter()
            .map(|e| self.view(e, current, Self::status_of(e)))
            .collect();
        let mut f = Map::new();
        f.insert("engines".into(), Value::Array(views));
        let builtin: Vec<&str> = ["kokoro", "onecore", "fake"]
            .into_iter()
            .filter(|b| self.registries().first().is_some_and(|r| r.get(b).is_ok()))
            .collect();
        f.insert("builtin".into(), json!(builtin));
        f.insert(
            "kinds".into(),
            json!(implemented_kinds()
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()),
        );
        f.insert(
            "presets".into(),
            json!(Preset::ALL.iter().map(Preset::as_str).collect::<Vec<_>>()),
        );
        f
    }

    /// The view of one profile.
    pub fn view_of(&self, id: &str, current: &str) -> Option<Value> {
        let entries = self.lock();
        let e = entries.iter().find(|e| e.id == id)?;
        Some(self.view(e, current, Self::status_of(e)))
    }

    fn check_secret(secret: &str) -> Result<(), Failure> {
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
        let profile = match Profile::from_json(&Value::Object(raw)) {
            Ok(p) => p,
            Err(ProfileError::Invalid(m)) => return Err(bad(m)),
            Err(ProfileError::Unsupported { kind, .. }) => {
                return Err(Failure::new(
                    Code::Unsupported,
                    format!("kind '{kind}' is not supported by this version"),
                ))
            }
        };
        let stored = profile.to_json();
        let id = profile.id.clone();
        let mut entries = self.lock();
        let at = entries.iter().position(|e| e.id == id);
        match at {
            Some(_) if !replace => {
                return Err(bad(format!("engine '{id}' exists; send replace: true")))
            }
            None if entries.len() >= MAX_PROFILES => {
                return Err(bad(format!("at most {MAX_PROFILES} engines")))
            }
            _ => {}
        }
        if let Some(s) = secret {
            self.keys
                .store()
                .set(&id, &Secret::new(s))
                .map_err(|e| Failure::new(Code::Engine, format!("cannot store the key: {e}")))?;
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

    /// `engine_remove` (after the caller moved off it when current).
    pub fn remove(&self, id: &str, forget_key: bool) -> Result<(), Failure> {
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
        let (key_ref, ext) = {
            let entries = self.lock();
            let e = entries
                .iter()
                .find(|e| e.id == id)
                .ok_or_else(|| Failure::new(Code::NotFound, format!("no engine '{id}'")))?;
            match &e.state {
                State::Ready(x) => (x.profile().key_ref.clone(), Some(x.clone())),
                _ => (
                    KeyRef::parse(e.raw.get("key_ref"))
                        .ok()
                        .flatten()
                        .unwrap_or(KeyRef::CredMan),
                    None,
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
                store.set(id, &Secret::new(s))
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

    pub fn file(&self) -> &Path {
        &self.file
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonara_engine::external::keys::MemoryStore;
    use sonara_engine::fake::FakeEngine;
    use sonara_engine::LicenseClass;

    fn home() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "sonarad-engines-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn setup(home: &Path, store: Arc<MemoryStore>) -> Setup {
        Setup {
            home: home.to_path_buf(),
            store,
            fallback: Some(Arc::new(FakeEngine::new())),
            fallback_voice: String::new(),
            default_engine: "fake".into(),
            log: None,
        }
    }

    fn registry() -> Arc<Registry> {
        Arc::new(Registry::new(&[
            LicenseClass::Permissive,
            LicenseClass::External,
        ]))
    }

    #[test]
    fn engines_json_round_trip_keeps_unknown_kinds_and_never_a_key() {
        let h = home();
        std::fs::write(
            h.join(FILE),
            r#"{"format": 1, "engines": [
                {"id": "el", "kind": "elevenlabs", "voice": "abc", "key_ref": "credman"},
                {"id": "bad", "kind": "openai-compatible", "url": "ftp://x"},
                {"id": "loc", "kind": "openai-compatible", "url": "http://127.0.0.1:9/v1",
                 "options": {"preset": "kokoro-fastapi"}}]}"#,
        )
        .unwrap();
        let store = Arc::new(MemoryStore::new());
        let (e, problems) = Engines::load(setup(&h, store.clone()));
        assert_eq!(problems.len(), 2, "{problems:?}");
        let reg = registry();
        e.attach(reg.clone());
        assert_eq!(
            reg.ids().iter().map(|i| i.as_str()).collect::<Vec<_>>(),
            vec!["loc"]
        );
        let list = e.list("loc");
        let views = list["engines"].as_array().unwrap();
        assert_eq!(views[0]["supported"], false);
        assert_eq!(views[1]["supported"], true);
        assert!(views[1]["error"].as_str().unwrap().contains("invalid url"));
        assert_eq!(views[2]["current"], true);
        assert_eq!(views[2]["model"], "kokoro", "the preset's default");
        assert_eq!(views[2]["local"], true);
        assert_eq!(views[2]["status"]["status"], "ready");
        assert_eq!(list["kinds"], json!(["openai-compatible"]));
        // A new profile with a secret: the file keeps the others and no key.
        e.add(
            &json!({"id": "openai", "kind": "openai-compatible",
                "options": {"preset": "openai"}, "api_key": "sk-in-the-wrong-place-123"}),
            Some("sk-secret-value-1234567890"),
            false,
        )
        .unwrap();
        let text = std::fs::read_to_string(h.join(FILE)).unwrap();
        assert!(!text.contains("sk-"), "{text}");
        assert!(text.contains("elevenlabs") && text.contains("ftp://x"));
        assert_eq!(
            store.get("openai").unwrap().unwrap().expose(),
            "sk-secret-value-1234567890"
        );
        assert!(reg.get("openai").is_ok());
        let (again, _) = Engines::load(setup(&h, store));
        assert_eq!(again.ids(), vec!["el", "bad", "loc", "openai"]);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn add_rules() {
        let h = home();
        let (e, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
        let local = json!({"id": "loc", "kind": "openai-compatible",
            "url": "http://127.0.0.1:9/v1", "options": {"preset": "kokoro-fastapi"}});
        assert!(!e.add(&local, None, false).unwrap());
        let err = e.add(&local, None, false).unwrap_err();
        assert_eq!(err.message, "engine 'loc' exists; send replace: true");
        assert!(e.add(&local, None, true).unwrap(), "a replace");
        let mut env = local.clone();
        env["id"] = json!("env1");
        env["key_ref"] = json!("env:MY_API_KEY");
        assert!(e
            .add(&env, Some("sk-x"), false)
            .unwrap_err()
            .message
            .contains("MY_API_KEY"));
        let mut none = local.clone();
        none["id"] = json!("n1");
        none["key_ref"] = json!("none");
        assert_eq!(
            e.add(&none, Some("k"), false).unwrap_err().code,
            Code::BadRequest
        );
        assert_eq!(
            e.add(
                &json!({"id": "kokoro", "kind": "openai-compatible"}),
                None,
                false
            )
            .unwrap_err()
            .message,
            "'kokoro' is a built-in engine"
        );
        assert_eq!(
            e.add(&json!({"id": "x", "kind": "azure"}), None, false)
                .unwrap_err()
                .code,
            Code::Unsupported
        );
        for i in 1..MAX_PROFILES {
            let mut p = local.clone();
            p["id"] = json!(format!("p{i}"));
            e.add(&p, None, false).unwrap();
        }
        let mut p = local.clone();
        p["id"] = json!("one-too-many");
        assert_eq!(
            e.add(&p, None, false).unwrap_err().message,
            "at most 16 engines"
        );
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn keys_and_removal() {
        let h = home();
        let store = Arc::new(MemoryStore::new());
        let (e, _) = Engines::load(setup(&h, store.clone()));
        let reg = registry();
        e.attach(reg.clone());
        e.add(
            &json!({"id": "c", "kind": "openai-compatible", "url": "https://tts.example.com/v1"}),
            None,
            false,
        )
        .unwrap();
        assert!(e.set_key("c", Some("sk-1")).unwrap());
        assert!(!e.set_key("c", None).unwrap());
        assert_eq!(e.set_key("nope", None).unwrap_err().code, Code::NotFound);
        e.set_key("c", Some("sk-2")).unwrap();
        e.remove("c", true).unwrap();
        assert!(store.get("c").unwrap().is_none(), "the key went with it");
        assert!(reg.get("c").is_err());
        assert_eq!(e.remove("c", true).unwrap_err().code, Code::NotFound);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn a_broken_file_is_kept_aside() {
        let h = home();
        std::fs::write(h.join(FILE), "{not json").unwrap();
        let (e, problems) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
        assert!(e.ids().is_empty());
        assert!(problems[0].contains("engines.json.bad"));
        assert_eq!(
            std::fs::read_to_string(h.join("engines.json.bad")).unwrap(),
            "{not json"
        );
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn notice_lines_follow_the_spec() {
        let n = Notice {
            engine: sonara_engine::EngineId::intern("openai"),
            reason: Some(Reason::Auth),
            status: Some(401),
            message: "OpenAI refused the key (401): bad key sk-abcdefghijklmnopqrstu".into(),
            fallback: Some(sonara_engine::EngineId("kokoro")),
        };
        assert_eq!(
            notice_line(&n),
            "engine openai fallback reason=auth status=401 -> kokoro: OpenAI refused the key \
             (401): bad key [redacted]"
        );
        let r = Notice { reason: None, ..n };
        assert_eq!(notice_line(&r), "engine openai recovered");
    }
}
