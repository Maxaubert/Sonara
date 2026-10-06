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
//! - Every profile's engine (and an unsaved one's, for its voices) shares
//!   one `Hold` (`hold`), which the server raises while Sonara is muted
//!   (`crate::quiet`, #227): nothing is sent to a provider meanwhile.
use crate::wire::{self, Code, Failure};
use serde_json::{json, Map, Value};
use sonara_engine::external::adapter::ModelInfo;
use sonara_engine::external::hold::Hold;
use sonara_engine::external::keys::{KeyResolver, KeyStore, MemoryStore, Secret, MAX_KEY_BYTES};
use sonara_engine::external::profile::{
    implemented_kinds, origin_of, KeyRef, Kind, Preset, Profile, ProfileError, MAX_PROFILES,
};
use sonara_engine::external::{External, ExternalConfig, Notice};
use sonara_engine::{Engine, Reason, Registry, Voice};
use sonara_system::LogFn;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

mod file;
mod notices;
mod registry;
#[cfg(test)]
mod tests;
mod view;

use file::{migrate, parse_file, read_text};
pub use notices::notice_line;
pub use registry::Models;

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
    /// Held while Sonara is muted: no profile sends anything.
    hold: Arc<Hold>,
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

/// The id an unsaved profile is built under (`voices` and `engine_models`
/// with `profile`).
pub const DRAFT_ID: &str = "draft";

/// A profile that fails `Profile::from_json` as a reply.
fn profile_failure(e: ProfileError) -> Failure {
    match e {
        ProfileError::Invalid(m) => bad(m),
        ProfileError::Unsupported { kind, .. } => Failure::new(
            Code::Unsupported,
            format!("kind '{kind}' is not supported by this version"),
        ),
    }
}

/// What `reload` changed.
#[derive(Debug, Default)]
pub struct Reloaded {
    /// For the log and the reply.
    pub problems: Vec<String>,
    /// Usable before, with another engine (or none) now.
    pub changed: Vec<String>,
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
            hold: Arc::new(Hold::new()),
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

    fn lock(&self) -> MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The mute every profile's engine follows (`crate::quiet`).
    pub fn hold(&self) -> &Arc<Hold> {
        &self.hold
    }

    pub fn default_engine(&self) -> &str {
        &self.default_engine
    }

    pub fn file(&self) -> &Path {
        &self.file
    }
}
