//! External engine profiles in this runtime (spec docs/plans/
//! 2026-10-04-external-engines-spec.md, sections 5 to 8): `engines.json` in
//! the home, one `External` engine per supported profile, registered in the
//! reader's and the previews' registries, keys in a `KeyStore`, and the
//! fallback notices as `sonarad.log` lines.
//!
//! - `engines.json` is read at start, before the reader starts, so a
//!   saved `engine` naming a profile works on the first sentence, and again
//!   on `engine_reload` (after the user or `sonara engines add --kind
//!   command` changed it). A file that is not JSON is copied to
//!   `engines.json.bad` and treated as empty until the next save (a reload
//!   refuses it and keeps what it has); an entry that fails validation, or
//!   of a kind this build lacks, is kept in the file, listed, and not
//!   registered.
//! - A `command` profile runs a program, so it comes only from the file:
//!   `add` (the protocol's `engine_add`, over TCP or HTTP, from any client
//!   or SDK) refuses to add one or to replace one with `E_FORBIDDEN`
//!   (security review of PR3). Removing, testing and selecting one stay
//!   allowed.
//! - Never a key in the file, a log line or a reply: secrets go only to the
//!   `KeyStore` (Credential Manager, or `fake-keys.json` with `--keys fake`).
//! - Keys are bound to the origin they were entered for (spec 6.4): a stored
//!   key goes with its origin, `engine_add` deletes one whose origin is not
//!   the profile's new one (unless a new secret comes with it), and an
//!   `env:` key goes only to the provider's default origin or the
//!   `key_origin` of its entry, which only the local file sets (the user, or
//!   the migration of a format 1 file at load).
use crate::wire::{self, Code, Failure};
use serde_json::{json, Map, Value};
use sonara_engine::external::keys::{KeyResolver, KeyStore, Secret, MAX_KEY_BYTES};
use sonara_engine::external::profile::{
    implemented_kinds, origin_of, KeyRef, Kind, Preset, Profile, ProfileError, MAX_PROFILES,
};
use sonara_engine::external::{External, ExternalConfig, Notice};
use sonara_engine::{Engine, Reason, Registry};
use sonara_system::LogFn;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const FILE: &str = "engines.json";
/// The `engines.json` format. 2: keys are bound to origins; a file of
/// format 1 (or none) is migrated at load (`migrate`).
pub const FORMAT: u64 = 2;
/// The field of an `env:` entry that confirms the origin its key may go to.
pub const KEY_ORIGIN: &str = "key_origin";
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
    /// The file's text when it was last read or saved (`None`: missing).
    /// A save first reloads a file changed since (the user's edit, or
    /// `sonara engines add`), so it never writes an older list over it.
    seen: Mutex<Option<String>>,
}

fn bad(m: impl Into<String>) -> Failure {
    Failure::new(Code::BadRequest, m)
}

/// The refusal of `engine_add` for a `command` profile.
pub const COMMAND_FORBIDDEN: &str = "a command engine runs a program on this PC, so it is \
     never added or changed over the protocol: add it with `sonara engines add <id> --kind \
     command`, or in engines.json";

fn is_command(raw: &Value) -> bool {
    raw.get("kind").and_then(Value::as_str) == Some(Kind::Command.as_str())
}

/// What `reload` changed.
#[derive(Debug, Default)]
pub struct Reloaded {
    /// For the log and the reply.
    pub problems: Vec<String>,
    /// Usable before, with another engine (or none) now.
    pub changed: Vec<String>,
}

/// The text of `engines.json` (`None` when it is missing).
fn read_text(file: &Path) -> Option<String> {
    std::fs::read_to_string(file).ok()
}

/// The `engines` array of `engines.json` (none when the file is missing)
/// and whether the file is of an older format (to migrate), or why the
/// file cannot be read.
fn parse_file(text: Option<&str>) -> Result<(Vec<Value>, bool), String> {
    match text {
        None => Ok((Vec::new(), false)),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(v) => Ok((
                v.get("engines")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                v.get("format").and_then(Value::as_u64).unwrap_or(1) < FORMAT,
            )),
            Err(e) => Err(format!("{FILE} is not valid JSON ({e})")),
        },
    }
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
        let text = read_text(&file);
        let (mut raws, legacy) = match parse_file(text.as_deref()) {
            Ok(read) => read,
            Err(why) => {
                let bad = setup.home.join(format!("{FILE}.bad"));
                let _ = std::fs::copy(&file, &bad);
                problems.push(format!(
                    "{why}; copied to {FILE}.bad, no external engines until the next change"
                ));
                (Vec::new(), false)
            }
        };
        if legacy {
            migrate(setup.store.as_ref(), &mut raws, &mut problems);
        }
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
            seen: Mutex::new(text),
        });
        let entries = engines.entries_from(raws, Vec::new(), &mut problems);
        if legacy {
            if let Err(e) = engines.save(&entries) {
                problems.push(e.message);
            }
        }
        *engines.lock() = entries;
        (engines, problems)
    }

    /// The entries of the file's profiles. An entry of `old` whose stored
    /// profile is the same is kept as it is (the same engine).
    fn entries_from(
        &self,
        raws: Vec<Value>,
        mut old: Vec<Entry>,
        problems: &mut Vec<String>,
    ) -> Vec<Entry> {
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
            if let Some(i) = old.iter().position(|e| e.id == id && e.raw == raw) {
                let kept = old.swap_remove(i);
                if let State::Invalid(m) = &kept.state {
                    problems.push(format!("{FILE}: engine '{id}' is not used: {m}"));
                }
                entries.push(kept);
                continue;
            }
            let state = match self.build(&raw) {
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
        entries
    }

    /// `engine_reload`: read `engines.json` again, the only way a `command`
    /// profile reaches a running runtime. It takes nothing from the
    /// request. An unchanged profile keeps its engine; a changed one is
    /// replaced in the registries, a gone or unusable one unregistered.
    /// A file that is not JSON changes nothing.
    pub fn reload(&self) -> Result<Reloaded, Failure> {
        let mut out = Reloaded::default();
        let mut entries = self.lock();
        let text = read_text(&self.file);
        let (raws, _) =
            parse_file(text.as_deref()).map_err(|why| bad(format!("{why}; nothing changed")))?;
        // An edit of the file is an edit path too (spec 6.4): a key whose
        // origin is no longer its entry's is deleted. A reload never binds
        // a key (only the migration at start does).
        for raw in &raws {
            self.drop_key_bound_elsewhere(raw);
        }
        *self.seen() = text;
        let old: Vec<Entry> = entries.drain(..).collect();
        let before: Vec<(String, Arc<External>)> = old
            .iter()
            .filter_map(|e| match &e.state {
                State::Ready(x) => Some((e.id.clone(), x.clone())),
                _ => None,
            })
            .collect();
        let next = self.entries_from(raws, old, &mut out.problems);
        *entries = next;
        let now: Vec<(String, Arc<External>)> = entries
            .iter()
            .filter_map(|e| match &e.state {
                State::Ready(x) => Some((e.id.clone(), x.clone())),
                _ => None,
            })
            .collect();
        drop(entries);
        let registries = self.registries();
        for (id, x) in &before {
            match now.iter().find(|(i, _)| i == id) {
                Some((_, y)) if Arc::ptr_eq(x, y) => {}
                Some((_, y)) => {
                    x.cancel();
                    for r in &registries {
                        let _ = r.replace(y.clone());
                    }
                    out.changed.push(id.clone());
                }
                None => {
                    x.cancel();
                    for r in &registries {
                        let _ = r.unregister(id);
                    }
                    out.changed.push(id.clone());
                }
            }
        }
        for (id, y) in &now {
            if !before.iter().any(|(i, _)| i == id) {
                for r in &registries {
                    if let Err(e) = r.register(y.clone()) {
                        self.note(&format!("engine {id}: not registered: {e}"));
                    }
                }
            }
        }
        for p in &out.problems {
            self.note(p);
        }
        self.note(&format!(
            "engine reload engines={} changed={}",
            self.lock().len(),
            out.changed.len()
        ));
        Ok(out)
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn seen(&self) -> MutexGuard<'_, Option<String>> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Before a save: take in what was written to the file since it was
    /// last read or saved (a lost update otherwise). A file that is not
    /// JSON is refused, never overwritten.
    /// Delete the stored key of `raw`'s id when it is bound to another
    /// origin than the entry's (or to none).
    fn drop_key_bound_elsewhere(&self, raw: &Value) {
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

    fn take_in_file_changes(&self) -> Result<(), Failure> {
        let changed = *self.seen() != read_text(&self.file);
        if changed {
            self.note(&format!(
                "{FILE} changed on disk; read again before the save"
            ));
            self.reload()?;
        }
        Ok(())
    }

    fn note(&self, line: &str) {
        sonara_system::log::emit(self.log.as_ref(), line);
    }

    fn build(&self, raw: &Value) -> Result<Arc<External>, ProfileError> {
        let mut profile = Profile::from_json(raw)?;
        if matches!(profile.key_ref, KeyRef::Env(_)) {
            profile.key_origin = raw
                .get(KEY_ORIGIN)
                .and_then(Value::as_str)
                .map(str::to_string);
        }
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
            "format": FORMAT,
            "engines": entries.iter().map(|e| e.raw.clone()).collect::<Vec<_>>(),
        });
        crate::config::write_json(&self.file, &v)
            .map_err(|e| Failure::new(Code::Engine, format!("cannot save {FILE}: {e}")))?;
        *self.seen() = read_text(&self.file);
        Ok(())
    }

    /// The profile view of spec 10.1 (never a secret).
    fn view(&self, e: &Entry, current: &str, status: Option<Value>) -> Value {
        let mut m = match &e.state {
            State::Ready(x) => {
                let p = x.profile();
                let mut m = p.to_json().as_object().cloned().unwrap_or_default();
                // What the profile itself sets, so an edit keeps the defaults
                // as defaults (an Azure region change still moves the URL).
                let explicit: Map<String, Value> = ["url", "model", "voice"]
                    .into_iter()
                    .filter_map(|k| m.get(k).map(|v| (k.to_string(), v.clone())))
                    .collect();
                m.insert("explicit".into(), Value::Object(explicit));
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
        let profile = match Profile::from_json(&Value::Object(raw)).and_then(|p| {
            // A new profile's program must exist now (a stored one whose
            // program is gone stays and reads with the fallback).
            p.check_new()?;
            Ok(p)
        }) {
            Ok(p) => p,
            Err(ProfileError::Invalid(m)) => return Err(bad(m)),
            Err(ProfileError::Unsupported { kind, .. }) => {
                return Err(Failure::new(
                    Code::Unsupported,
                    format!("kind '{kind}' is not supported by this version"),
                ))
            }
        };
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

    pub fn file(&self) -> &Path {
        &self.file
    }
}

/// Bind what a format 1 file left unbound to the entry's origin now (the
/// file is local, so its addresses are the user's): Credential Manager keys
/// without an origin, and `env:` entries without `key_origin`. Problems are
/// for the log; a key that cannot be bound stays unbound and is not sent.
fn migrate(store: &dyn KeyStore, raws: &mut [Value], problems: &mut Vec<String>) {
    for raw in raws.iter_mut() {
        let Some(id) = raw.get("id").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        let origin = origin_of(raw);
        let env = raw
            .get("key_ref")
            .and_then(Value::as_str)
            .is_some_and(|k| k.starts_with("env:"));
        if let (true, Some(o), Some(m)) = (env, &origin, raw.as_object_mut()) {
            m.entry(KEY_ORIGIN).or_insert_with(|| json!(o));
        }
        if let Ok(Some(k)) = store.get(&id) {
            if k.origin.is_none() {
                let done = match &origin {
                    Some(o) => store.set(&id, &k.secret, o).map_err(|e| e.to_string()),
                    None => Err("the engine has no address".into()),
                };
                if let Err(e) = done {
                    problems.push(format!(
                        "engine '{id}': its key could not be bound to an address ({e}); \
                         enter it again"
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonara_engine::external::keys::{KeyStore, MemoryStore, Secret};
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
                {"id": "ca", "kind": "future-kind", "voice": "abc", "key_ref": "credman"},
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
        assert_eq!(
            list["kinds"],
            json!([
                "openai-compatible",
                "elevenlabs",
                "azure",
                "google",
                "cartesia",
                "deepgram",
                "command"
            ])
        );
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
        assert!(text.contains("future-kind") && text.contains("ftp://x"));
        assert_eq!(
            store.get("openai").unwrap().unwrap().expose(),
            "sk-secret-value-1234567890"
        );
        assert!(reg.get("openai").is_ok());
        let (again, _) = Engines::load(setup(&h, store));
        assert_eq!(again.ids(), vec!["ca", "bad", "loc", "openai"]);
        let _ = std::fs::remove_dir_all(&h);
    }

    /// `sonara engines add` writes engines.json as the user and then
    /// reloads; a protocol add or remove that saves in between must not
    /// write its older list over the user's new entry.
    #[test]
    fn a_save_never_loses_an_entry_written_to_the_file_meanwhile() {
        let h = home();
        let loc = |id: &str| {
            json!({"id": id, "kind": "openai-compatible",
                "url": "http://127.0.0.1:9/v1", "options": {"preset": "kokoro-fastapi"}})
        };
        let (e, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
        let reg = registry();
        e.attach(reg.clone());
        e.add(&loc("a"), None, false).unwrap();
        // The user's own edit (the CLI's write), not yet reloaded.
        let write_with = |ids: &[&str]| {
            let engines: Vec<Value> = ids.iter().map(|i| loc(i)).collect();
            std::fs::write(
                h.join(FILE),
                json!({"format": 1, "engines": engines}).to_string(),
            )
            .unwrap();
        };
        write_with(&["a", "mine"]);
        e.add(&loc("b"), None, false).unwrap();
        let (again, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
        assert_eq!(again.ids(), vec!["a", "mine", "b"]);
        assert!(
            reg.get("mine").is_ok(),
            "the user's entry is registered too"
        );
        write_with(&["a", "mine", "b", "mine2"]);
        e.remove("a", false).unwrap();
        let (again, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
        assert_eq!(again.ids(), vec!["mine", "b", "mine2"]);
        // A file the user broke is never overwritten by a protocol save.
        std::fs::write(h.join(FILE), "{ not json").unwrap();
        let err = e.add(&loc("c"), None, false).unwrap_err();
        assert!(err.message.contains("not valid JSON"), "{}", err.message);
        assert_eq!(std::fs::read_to_string(h.join(FILE)).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(&h);
    }

    /// The view fills in the values in force, and `explicit` keeps what the
    /// profile itself sets, so an edit form does not pin the defaults.
    #[test]
    fn the_view_tells_explicit_values_from_defaults() {
        let h = home();
        let (e, _) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
        e.add(
            &json!({"id": "openai", "kind": "openai-compatible", "options": {"preset": "openai"}}),
            None,
            false,
        )
        .unwrap();
        e.add(
            &json!({"id": "az", "kind": "azure", "voice": "en-US-AvaMultilingualNeural",
                "options": {"region": "westeurope"}}),
            None,
            false,
        )
        .unwrap();
        e.add(
            &json!({"id": "own", "kind": "openai-compatible", "url": "http://127.0.0.1:9/v1",
                "model": "m1", "voice": "v1", "options": {"preset": "generic"}}),
            None,
            false,
        )
        .unwrap();
        let list = e.list("");
        let views = list["engines"].as_array().unwrap();
        assert_eq!(views[0]["url"], "https://api.openai.com/v1");
        assert_eq!(views[0]["explicit"], json!({}));
        assert_eq!(
            views[1]["url"],
            "https://westeurope.tts.speech.microsoft.com"
        );
        assert_eq!(
            views[1]["explicit"],
            json!({"voice": "en-US-AvaMultilingualNeural"})
        );
        assert_eq!(
            views[2]["explicit"],
            json!({"url": "http://127.0.0.1:9/v1", "model": "m1", "voice": "v1"})
        );
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
            e.add(&json!({"id": "x", "kind": "future-kind"}), None, false)
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

    /// A stored profile whose program went away stays registered (it reads
    /// with the fallback until the program is back). A command profile is
    /// never added through `add` (the protocol): only from the file.
    #[test]
    fn command_profiles_come_only_from_the_file() {
        let h = home();
        let missing = "C:\\Nope\\sonara-missing-tts.exe";
        let exe = std::env::current_exe().unwrap().display().to_string();
        std::fs::write(
            h.join(FILE),
            json!({"format": 1, "engines": [
                {"id": "gone", "kind": "command", "options": {"argv": [missing]}},
                {"id": "prog", "kind": "command", "options": {"argv": [exe]}}]})
            .to_string(),
        )
        .unwrap();
        let (e, problems) = Engines::load(setup(&h, Arc::new(MemoryStore::new())));
        assert!(problems.is_empty(), "{problems:?}");
        let reg = registry();
        e.attach(reg.clone());
        assert!(reg.get("gone").is_ok());
        for (p, replace) in [
            (
                json!({"id": "typo", "kind": "command", "options": {"argv": [missing]}}),
                false,
            ),
            (
                json!({"id": "prog", "kind": "command", "options": {"argv": [exe]}}),
                true,
            ),
            (
                json!({"id": "prog", "kind": "openai-compatible",
                    "url": "http://127.0.0.1:9/v1", "key_ref": "none"}),
                true,
            ),
        ] {
            let err = e.add(&p, None, replace).unwrap_err();
            assert_eq!(err.code, Code::Forbidden, "{p}");
        }
        assert_eq!(e.ids(), vec!["gone", "prog"]);
        let list = e.list("prog");
        let view = &list["engines"].as_array().unwrap()[1];
        let name = std::path::Path::new(&exe)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(view["sends_text_to"], format!("program {name}"));
        assert_eq!(view["local"], true);
        assert_eq!(view["key_ref"], "none");
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn keys_of_a_format_1_file_are_bound_to_its_origins_at_load() {
        let h = home();
        let var = format!("SONARA_TEST_LOAD_{}_API_KEY", std::process::id());
        std::fs::write(
            h.join(FILE),
            format!(
                r#"{{"format": 1, "engines": [
                {{"id": "c", "kind": "openai-compatible", "url": "https://tts.example.com/v1",
                 "key_ref": "credman"}},
                {{"id": "e", "kind": "openai-compatible", "url": "https://env.example.com/v1",
                 "key_ref": "env:{var}"}},
                {{"id": "el", "kind": "elevenlabs", "voice": "abc", "key_ref": "credman"}}]}}"#
            ),
        )
        .unwrap();
        let store = Arc::new(MemoryStore::new());
        store.set_unbound("c", &Secret::new("sk-c"));
        store.set_unbound("el", &Secret::new("sk-el"));
        std::env::set_var(&var, "sk-env");
        let (e, problems) = Engines::load(setup(&h, store.clone()));
        assert!(
            problems.iter().all(|p| !p.contains("bound")),
            "{problems:?}"
        );
        assert_eq!(
            store.get("c").unwrap().unwrap().origin.as_deref(),
            Some("https://tts.example.com:443")
        );
        assert_eq!(
            store.get("el").unwrap().unwrap().origin.as_deref(),
            Some("https://api.elevenlabs.io:443"),
            "a kind this build lacks is bound to its provider"
        );
        assert!(e.get("c").unwrap().key_present());
        assert!(
            e.get("e").unwrap().key_present(),
            "the local file confirms it"
        );
        let file: Value =
            serde_json::from_str(&std::fs::read_to_string(h.join(FILE)).unwrap()).unwrap();
        assert_eq!(file["format"], FORMAT);
        assert_eq!(
            file["engines"][1]["key_origin"],
            "https://env.example.com:443"
        );
        assert!(file["engines"][0].get("key_origin").is_none());
        // Loaded again (format 2 now), nothing changes.
        let (again, _) = Engines::load(setup(&h, store.clone()));
        assert!(again.get("e").unwrap().key_present());
        std::env::remove_var(&var);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn a_format_2_file_binds_nothing_new() {
        let h = home();
        let var = format!("SONARA_TEST_LOAD2_{}_API_KEY", std::process::id());
        std::fs::write(
            h.join(FILE),
            format!(
                r#"{{"format": 2, "engines": [
                {{"id": "c", "kind": "openai-compatible", "url": "https://tts.example.com/v1",
                 "key_ref": "credman"}},
                {{"id": "e", "kind": "openai-compatible", "url": "https://env.example.com/v1",
                 "key_ref": "env:{var}"}},
                {{"id": "ok", "kind": "openai-compatible", "url": "https://env.example.com/v1",
                 "key_ref": "env:{var}", "key_origin": "https://env.example.com:443"}}]}}"#
            ),
        )
        .unwrap();
        let store = Arc::new(MemoryStore::new());
        // A key without an origin (an older runtime wrote it after the
        // migration) stays unbound and unused.
        store.set_unbound("c", &Secret::new("sk-c"));
        std::env::set_var(&var, "sk-env");
        let (e, _) = Engines::load(setup(&h, store.clone()));
        assert!(store.get("c").unwrap().unwrap().origin.is_none());
        assert!(!e.get("c").unwrap().key_present());
        assert!(!e.get("e").unwrap().key_present(), "not confirmed");
        assert!(e.get("ok").unwrap().key_present(), "confirmed in the file");
        // A replace over the protocol that keeps the origin keeps the
        // confirmation; one that changes it drops it.
        let mut p = e.view_of("ok", "").unwrap();
        p["voice"] = json!("other");
        e.add(&p, None, true).unwrap();
        assert!(e.get("ok").unwrap().key_present());
        p["url"] = json!("https://evil.example.com/v1");
        e.add(&p, None, true).unwrap();
        assert!(!e.get("ok").unwrap().key_present());
        p["url"] = json!("https://env.example.com/v1");
        e.add(&p, None, true).unwrap();
        assert!(
            !e.get("ok").unwrap().key_present(),
            "a confirmation dropped over the protocol is not restored by it"
        );
        std::env::remove_var(&var);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn engine_key_binds_to_the_entrys_origin() {
        let h = home();
        std::fs::write(
            h.join(FILE),
            r#"{"format": 2, "engines": [
                {"id": "az", "kind": "azure", "voice": "en-US-AvaNeural",
                 "options": {"region": "westeurope"}}]}"#,
        )
        .unwrap();
        let store = Arc::new(MemoryStore::new());
        let (e, _) = Engines::load(setup(&h, store.clone()));
        e.set_key("az", Some("k-az")).unwrap();
        assert_eq!(
            store.get("az").unwrap().unwrap().origin.as_deref(),
            Some("https://westeurope.tts.speech.microsoft.com:443")
        );
        let (e, _) = Engines::load(setup(&h, store.clone()));
        e.add(
            &json!({"id": "c", "kind": "openai-compatible", "url": "https://a.example.com/v1"}),
            Some("sk-a"),
            false,
        )
        .unwrap();
        assert_eq!(
            store.get("c").unwrap().unwrap().origin.as_deref(),
            Some("https://a.example.com:443")
        );
        // A replace to another origin without a secret deletes the key.
        e.add(
            &json!({"id": "c", "kind": "openai-compatible", "url": "https://b.example.com/v1"}),
            None,
            true,
        )
        .unwrap();
        assert!(store.get("c").unwrap().is_none());
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
