//! Persisted settings (#201): `<home>\config.json` and
//! `<home>\session_prefs.json`.
//!
//! - **One schema** ([`SCHEMA`]): every setting a client can `set` that
//!   outlives the runtime, with its default and its validation. The layers
//!   validate protocol requests themselves; the schema validates what comes
//!   from disk (a hand-edited file, the migration from the Python plugin)
//!   and its defaults equal the layers' (a test checks it).
//! - **Only the user's keys** are written (the Python M7 rule): a key is
//!   stored once a client set it (even to the default) and never before,
//!   so a later release can change a default the user never chose.
//!   `summaries` stores only the fields that were set, plus the custom
//!   prompt of each style (`summaries.prompts`).
//! - **Atomic writes** (a temp file, then a rename) under the store's lock,
//!   so two changes never interleave on disk. A file that cannot be read
//!   or parsed gives the defaults; a value the schema refuses is dropped
//!   and reported, never fatal.
//! - **Per-channel preferences** (`session_prefs.json`): `label`, `voice`
//!   and `muted` per channel id (the Claude session id), the most recent
//!   [`PREFS_CAP`] kept. The label replaces the one the client sends; a
//!   muted channel is held unread (L2 `set_muted`, #196); the voice is
//!   stored for the settings page.
//!
//! Keys starting with `_` (the migration marker) are kept as they are.
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

pub const CONFIG_FILE: &str = "config.json";
pub const PREFS_FILE: &str = "session_prefs.json";
/// The migration marker in `config.json` (see `migrate`).
pub const MARKER: &str = "_migrated";
/// Channel preferences kept (the most recently changed).
pub const PREFS_CAP: usize = 200;
/// Longest channel label, in characters (as the Python plugin).
pub const LABEL_MAX: usize = 60;

/// Which layer a setting belongs to: it is applied when that layer starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Reader,
    Channels,
    Agent,
    System,
    /// `sonarad` itself (the troubleshooting log).
    Host,
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    /// An integer in this range.
    Range(u64, u64),
    /// One of these strings.
    OneOf(&'static [&'static str]),
    /// A non-blank string.
    Text,
    /// A non-blank string or null.
    TextOrNull,
    /// The `summaries` object (see `validate_summaries`).
    Summaries,
    /// true or false.
    Bool,
}

/// One persisted setting.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub key: &'static str,
    pub layer: Layer,
    kind: Kind,
    default: &'static str,
}

pub const AUDIO_MODES: &[&str] = &["off", "duck", "pause"];
pub const VERBOSITY: &[&str] = &["everything", "skip_code"];
/// Old names of a value, accepted and stored as the new one: the
/// verbosity levels before #214 (and the Python plugin's).
const ALIASES: &[(&str, &str, &str)] = &[
    ("verbosity", "all", "everything"),
    ("verbosity", "medium", "skip_code"),
    ("verbosity", "quiet", "skip_code"),
];

fn canonical<'a>(key: &str, v: &'a str) -> &'a str {
    ALIASES
        .iter()
        .find(|(k, old, _)| *k == key && *old == v)
        .map(|(_, _, now)| *now)
        .unwrap_or(v)
}
pub const STYLES: &[&str] = &["tidy", "natural", "brief"];
pub const COMMANDS: &[&str] = &["claude", "codex"];
pub const BACKGROUND: &[&str] = &["all", "earcon_only"];
const ON_OFF: &[&str] = &["on", "off"];

/// Every persisted setting, its layer, validation and default (as JSON).
///
/// The defaults are the product's (#202, the maintainer's settings of the
/// Python plugin): Kokoro's `af_sarah` at 250 words per minute, verbosity
/// `skip_code` (#214; it was `medium`), prose held until five chunks wait,
/// every session read, media paused while Sonara speaks, summaries off,
/// unmuted. `sonarad` applies
/// them to the layers (L1 in `apply_reader`), so they differ from the
/// library crates' own defaults on purpose.
pub const SCHEMA: &[Setting] = &[
    Setting {
        key: "engine",
        layer: Layer::Reader,
        kind: Kind::Text,
        default: "\"onecore\"",
    },
    Setting {
        key: "voice",
        layer: Layer::Reader,
        kind: Kind::TextOrNull,
        default: "\"af_sarah\"",
    },
    Setting {
        key: "rate",
        layer: Layer::Reader,
        kind: Kind::Range(100, 400),
        default: "250",
    },
    Setting {
        key: "volume",
        layer: Layer::Reader,
        kind: Kind::Range(0, 100),
        default: "100",
    },
    Setting {
        key: "channel_announce",
        layer: Layer::Channels,
        kind: Kind::OneOf(ON_OFF),
        default: "\"on\"",
    },
    Setting {
        key: "mute_level",
        layer: Layer::Agent,
        kind: Kind::Range(0, 2),
        default: "0",
    },
    Setting {
        key: "verbosity",
        layer: Layer::Agent,
        kind: Kind::OneOf(VERBOSITY),
        default: "\"skip_code\"",
    },
    Setting {
        key: "minqueue",
        layer: Layer::Agent,
        kind: Kind::Range(0, 10),
        default: "5",
    },
    Setting {
        key: "background_policy",
        layer: Layer::Agent,
        kind: Kind::OneOf(BACKGROUND),
        default: "\"all\"",
    },
    Setting {
        key: "summaries",
        layer: Layer::Agent,
        kind: Kind::Summaries,
        default: "{\"enabled\": false, \"command\": \"claude\", \"model\": \"haiku\", \
                  \"timeout\": 60, \"settle_ms\": 600, \"style\": \"natural\", \"prompts\": {}}",
    },
    Setting {
        key: "audio_mode",
        layer: Layer::System,
        kind: Kind::OneOf(AUDIO_MODES),
        default: "\"pause\"",
    },
    Setting {
        key: "duck_level",
        layer: Layer::System,
        kind: Kind::Range(0, 100),
        default: "30",
    },
    // The troubleshooting log records what is read and what hooks send
    // (#219). On by default for now, at the user's request.
    Setting {
        key: "debug_log",
        layer: Layer::Host,
        kind: Kind::Bool,
        default: "true",
    },
];

pub fn setting(key: &str) -> Option<&'static Setting> {
    SCHEMA.iter().find(|s| s.key == key)
}

/// The default of a persisted setting.
pub fn default(key: &str) -> Option<Value> {
    setting(key).map(|s| serde_json::from_str(s.default).expect("schema defaults are JSON"))
}

/// A whole number, also from a float with no fraction (a hand-edited file).
fn whole(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| {
        v.as_f64()
            .filter(|f| f.is_finite() && *f >= 0.0 && f.fract() == 0.0)
            .map(|f| f as u64)
    })
}

fn non_blank(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The value to store for `key`, or why it is refused.
pub fn validate(key: &str, v: &Value) -> Result<Value, String> {
    let s = setting(key).ok_or_else(|| format!("'{key}' is not a persisted setting"))?;
    match s.kind {
        Kind::Range(lo, hi) => whole(v)
            .filter(|n| (lo..=hi).contains(n))
            .map(|n| json!(n))
            .ok_or_else(|| format!("'{key}' is an integer {lo} to {hi}")),
        Kind::OneOf(options) => v
            .as_str()
            .map(|s| canonical(key, s))
            .filter(|s| options.contains(s))
            .map(|s| json!(s))
            .ok_or_else(|| format!("'{key}' is one of {}", options.join(", "))),
        Kind::Text => non_blank(v)
            .map(Value::String)
            .ok_or_else(|| format!("'{key}' is a non-empty string")),
        Kind::TextOrNull => match v {
            Value::Null => Ok(Value::Null),
            other => non_blank(other)
                .map(Value::String)
                .ok_or_else(|| format!("'{key}' is a non-empty string or null")),
        },
        Kind::Summaries => validate_summaries(v).map(Value::Object),
        Kind::Bool => v
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| format!("'{key}' is true or false")),
    }
}

/// The custom prompts: style -> non-blank instruction.
fn validate_prompts(v: &Value) -> Result<Map<String, Value>, String> {
    let o = v
        .as_object()
        .ok_or("'summaries.prompts' is an object of style: instruction")?;
    let mut out = Map::new();
    for (style, text) in o {
        if !STYLES.contains(&style.as_str()) {
            return Err(format!(
                "'summaries.prompts' has an unknown style '{style}'"
            ));
        }
        if let Some(t) = non_blank(text) {
            out.insert(style.clone(), Value::String(t));
        }
    }
    Ok(out)
}

/// The fields of `summaries` that are set, validated: `enabled`,
/// `command`, `model`, `timeout` (15 to 300 s), `settle_ms` (0 to 5000),
/// `style`, `prompts`. Unknown fields are dropped; `prompt` (the custom
/// instruction of the current style, derived from `prompts`) is never
/// stored.
pub fn validate_summaries(v: &Value) -> Result<Map<String, Value>, String> {
    let o = v.as_object().ok_or("'summaries' is an object")?;
    let mut out = Map::new();
    for (field, value) in o {
        let clean = match field.as_str() {
            "enabled" => value
                .as_bool()
                .map(Value::Bool)
                .ok_or("'summaries.enabled' is true or false")?,
            "command" => value
                .as_str()
                .filter(|s| COMMANDS.contains(s))
                .map(|s| json!(s))
                .ok_or("'summaries.command' is \"claude\" or \"codex\"")?,
            "model" => non_blank(value)
                .map(Value::String)
                .ok_or("'summaries.model' is a non-empty string")?,
            "timeout" => whole(value)
                .filter(|n| (15..=300).contains(n))
                .map(|n| json!(n))
                .ok_or("'summaries.timeout' is 15 to 300 seconds")?,
            "settle_ms" => whole(value)
                .filter(|n| *n <= 5_000)
                .map(|n| json!(n))
                .ok_or("'summaries.settle_ms' is 0 to 5000 milliseconds")?,
            "style" => value
                .as_str()
                .filter(|s| STYLES.contains(s))
                .map(|s| json!(s))
                .ok_or("'summaries.style' is \"tidy\", \"natural\" or \"brief\"")?,
            "prompts" => Value::Object(validate_prompts(value)?),
            _ => continue,
        };
        out.insert(field.clone(), clean);
    }
    Ok(out)
}

/// One channel's preferences.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prefs {
    /// The name read on a switch; replaces the client's label.
    pub label: Option<String>,
    /// A voice for this channel (stored for the settings page; not applied
    /// yet).
    pub voice: Option<String>,
    /// The channel's speech is held unread (#196).
    pub muted: bool,
}

impl Prefs {
    pub fn is_empty(&self) -> bool {
        self.label.is_none() && self.voice.is_none() && !self.muted
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        if let Some(l) = &self.label {
            m.insert("label".into(), json!(l));
        }
        if let Some(v) = &self.voice {
            m.insert("voice".into(), json!(v));
        }
        if self.muted {
            m.insert("muted".into(), json!(true));
        }
        Value::Object(m)
    }

    /// From a stored entry: unknown fields and bad values are dropped. The
    /// Python plugin's `name` is read as `label`.
    pub fn from_json(v: &Value) -> Prefs {
        let label = v
            .get("label")
            .or_else(|| v.get("name"))
            .and_then(non_blank)
            .map(|l| clip(&l));
        Prefs {
            label,
            voice: v.get("voice").and_then(non_blank),
            muted: v.get("muted").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn clip(label: &str) -> String {
    label.chars().take(LABEL_MAX).collect()
}

/// A change to one channel's preferences: `None` leaves a field alone;
/// `Some(None)` (or an empty string) clears a label or voice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrefsUpdate {
    pub label: Option<Option<String>>,
    pub voice: Option<Option<String>>,
    pub muted: Option<bool>,
}

struct Inner {
    /// `config.json`: the user's keys (and the marker).
    user: Map<String, Value>,
    /// Keys of `config.json` this runtime does not use: unknown ones (from a
    /// newer release) and values the schema refuses (a hand edit). They are
    /// written back unchanged, so a save never erases them; setting the key
    /// replaces a refused value.
    extra: Map<String, Value>,
    /// `session_prefs.json`, oldest change first, with the time of the
    /// change (milliseconds since 1970, `changed` in the file: a JSON
    /// object keeps no order).
    prefs: Vec<(String, Prefs, u64)>,
    /// The label each channel's client gave it (memory only), so a page
    /// can show it next to the user's own name for it.
    client_labels: HashMap<String, String>,
}

/// The persisted settings of one home (or of none: `memory`).
pub struct Store {
    dir: Option<PathBuf>,
    inner: Mutex<Inner>,
}

/// Read a JSON object file. A file that exists but is not a JSON object is
/// copied to `<name>.bad` first when `backup` is set, so the next save
/// (which replaces it) does not lose what the user had.
fn read_object(path: &Path, problems: &mut Vec<String>, backup: bool) -> Map<String, Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Map::new(),
        Err(e) => {
            problems.push(format!("cannot read {}: {e}", path.display()));
            return Map::new();
        }
    };
    match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
        Ok(Value::Object(m)) => m,
        _ => {
            let mut note = format!(
                "{} is not a JSON object; using the defaults",
                path.display()
            );
            if backup {
                let mut bad = path.as_os_str().to_owned();
                bad.push(".bad");
                let bad = PathBuf::from(bad);
                match std::fs::write(&bad, &text) {
                    Ok(()) => note.push_str(&format!(" (a copy is in {})", bad.display())),
                    Err(e) => {
                        note.push_str(&format!(" (cannot copy it to {}: {e})", bad.display()))
                    }
                }
            }
            problems.push(note);
            Map::new()
        }
    }
}

/// Write JSON atomically: a temp file next to it, then a rename (retried
/// briefly: a reader holding the file can make it fail for a moment on
/// Windows).
pub fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    sonara_system::state_file::write(path, value)
}

impl Store {
    /// A store that never touches the disk (tests, embedders).
    pub fn memory() -> Arc<Store> {
        Arc::new(Store {
            dir: None,
            inner: Mutex::new(Inner {
                user: Map::new(),
                extra: Map::new(),
                prefs: Vec::new(),
                client_labels: HashMap::new(),
            }),
        })
    }

    /// Load the files of the home `dir`. Returns the store and the problems
    /// found (values dropped, unreadable files), for the log.
    pub fn load(dir: &Path) -> (Arc<Store>, Vec<String>) {
        let mut problems = Vec::new();
        let raw = read_object(&dir.join(CONFIG_FILE), &mut problems, true);
        let mut user = Map::new();
        let mut extra = Map::new();
        for (key, value) in raw {
            if key.starts_with('_') {
                user.insert(key, value);
                continue;
            }
            match validate(&key, &value) {
                Ok(v) => {
                    user.insert(key, v);
                }
                Err(e) if setting(&key).is_some() => {
                    problems.push(format!(
                        "config.json: {e}; using the default (the value is kept in the file)"
                    ));
                    extra.insert(key, value);
                }
                Err(_) => {
                    problems.push(format!(
                        "config.json: unknown key '{key}' ignored (kept in the file)"
                    ));
                    extra.insert(key, value);
                }
            }
        }
        let raw = read_object(&dir.join(PREFS_FILE), &mut problems, false);
        let mut prefs: Vec<(String, Prefs, u64)> = raw
            .iter()
            .filter(|(id, _)| !id.is_empty())
            .map(|(id, v)| {
                let at = v.get("changed").and_then(Value::as_u64).unwrap_or(0);
                (id.clone(), Prefs::from_json(v), at)
            })
            .filter(|(_, p, _)| !p.is_empty())
            .collect();
        prefs.sort_by_key(|(_, _, at)| *at);
        let excess = prefs.len().saturating_sub(PREFS_CAP);
        prefs.drain(..excess);
        let store = Store {
            dir: Some(dir.to_path_buf()),
            inner: Mutex::new(Inner {
                user,
                extra,
                prefs,
                client_labels: HashMap::new(),
            }),
        };
        (Arc::new(store), problems)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn config_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(CONFIG_FILE))
    }

    /// What the user set for `key`, if anything.
    pub fn user(&self, key: &str) -> Option<Value> {
        self.lock().user.get(key).cloned()
    }

    /// The user's value for `key`, else its default.
    pub fn value(&self, key: &str) -> Value {
        self.user(key)
            .or_else(|| default(key))
            .unwrap_or(Value::Null)
    }

    /// The user's keys as stored (for tests and the page).
    pub fn user_keys(&self) -> Map<String, Value> {
        self.lock().user.clone()
    }

    fn save_config(&self, inner: &Inner) {
        if let Some(dir) = &self.dir {
            let path = dir.join(CONFIG_FILE);
            let mut all = inner.extra.clone();
            all.extend(inner.user.clone());
            if let Err(e) = write_json(&path, &Value::Object(all)) {
                eprintln!("sonarad: cannot write {}: {e}", path.display());
            }
        }
    }

    fn save_prefs(&self, inner: &Inner) {
        if let Some(dir) = &self.dir {
            let map: Map<String, Value> = inner
                .prefs
                .iter()
                .map(|(id, p, at)| {
                    let mut v = p.to_json();
                    v["changed"] = json!(at);
                    (id.clone(), v)
                })
                .collect();
            let path = dir.join(PREFS_FILE);
            if let Err(e) = write_json(&path, &Value::Object(map)) {
                eprintln!("sonarad: cannot write {}: {e}", path.display());
            }
        }
    }

    /// The user set `key` to `value` (the value now in force). A value the
    /// schema refuses is not stored (and reported).
    pub fn record(&self, key: &str, value: &Value) {
        if key == "summaries" {
            if let Some(o) = value.as_object() {
                self.record_summaries(o);
            }
            return;
        }
        match validate(key, value) {
            Ok(v) => {
                let mut inner = self.lock();
                let refused = inner.extra.remove(key).is_some();
                if inner.user.get(key) == Some(&v) && !refused {
                    return;
                }
                inner.user.insert(key.to_string(), v);
                self.save_config(&inner);
            }
            Err(e) => eprintln!("sonarad: not saved: {e}"),
        }
    }

    /// Forget the user's value of `key` (the default applies again).
    pub fn forget(&self, key: &str) {
        let mut inner = self.lock();
        let refused = inner.extra.remove(key).is_some();
        if inner.user.remove(key).is_some() || refused {
            self.save_config(&inner);
        }
    }

    /// Merge the `summaries` fields that were set. `prompts` replaces the
    /// stored prompts as a whole (the caller passes the merged map); an
    /// empty map removes them.
    pub fn record_summaries(&self, fields: &Map<String, Value>) {
        let clean = match validate_summaries(&Value::Object(fields.clone())) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("sonarad: not saved: {e}");
                return;
            }
        };
        let mut inner = self.lock();
        let mut stored = inner
            .user
            .get("summaries")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let before = stored.clone();
        let refused = inner.extra.remove("summaries").is_some();
        for (k, v) in clean {
            if k == "prompts" && v.as_object().is_some_and(Map::is_empty) {
                stored.remove("prompts");
            } else {
                stored.insert(k, v);
            }
        }
        if stored == before && inner.user.contains_key("summaries") && !refused {
            return;
        }
        if stored.is_empty() {
            inner.user.remove("summaries");
        } else {
            inner.user.insert("summaries".into(), Value::Object(stored));
        }
        self.save_config(&inner);
    }

    /// The `summaries` fields the user set.
    pub fn summaries(&self) -> Map<String, Value> {
        self.user("summaries")
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }

    /// The custom prompt of each style.
    pub fn prompts(&self) -> BTreeMap<String, String> {
        self.summaries()
            .get("prompts")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One channel's preferences.
    pub fn prefs(&self, channel: &str) -> Prefs {
        self.lock()
            .prefs
            .iter()
            .find(|(id, _, _)| id == channel)
            .map(|(_, p, _)| p.clone())
            .unwrap_or_default()
    }

    /// Every channel with preferences, oldest change first.
    pub fn all_prefs(&self) -> Vec<(String, Prefs)> {
        self.lock()
            .prefs
            .iter()
            .map(|(id, p, _)| (id.clone(), p.clone()))
            .collect()
    }

    /// Change one channel's preferences; returns them as now stored.
    pub fn set_prefs(&self, channel: &str, update: PrefsUpdate) -> Prefs {
        let mut inner = self.lock();
        let mut p = inner
            .prefs
            .iter()
            .position(|(id, _, _)| id == channel)
            .map(|i| inner.prefs.remove(i).1)
            .unwrap_or_default();
        let text = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if let Some(l) = update.label {
            p.label = text(l).map(|l| clip(&l));
        }
        if let Some(v) = update.voice {
            p.voice = text(v);
        }
        if let Some(m) = update.muted {
            p.muted = m;
        }
        if !p.is_empty() {
            let last = inner.prefs.last().map(|(_, _, at)| *at).unwrap_or(0);
            // Strictly increasing, so the order survives a reload.
            let at = now_ms().max(last + 1);
            inner.prefs.push((channel.to_string(), p.clone(), at));
            let excess = inner.prefs.len().saturating_sub(PREFS_CAP);
            inner.prefs.drain(..excess);
        }
        self.save_prefs(&inner);
        p
    }

    /// Forget one channel's preferences (the settings page's forget).
    pub fn forget_prefs(&self, channel: &str) {
        let mut inner = self.lock();
        let before = inner.prefs.len();
        inner.prefs.retain(|(id, _, _)| id != channel);
        inner.client_labels.remove(channel);
        if inner.prefs.len() != before {
            self.save_prefs(&inner);
        }
    }

    /// The label a client gave a channel (`channel_open`).
    pub fn note_client_label(&self, channel: &str, label: Option<&str>) {
        let mut inner = self.lock();
        match label.filter(|l| !l.is_empty()) {
            Some(l) => {
                inner
                    .client_labels
                    .insert(channel.to_string(), l.to_string());
            }
            None => {
                inner.client_labels.remove(channel);
            }
        }
    }

    pub fn client_label(&self, channel: &str) -> Option<String> {
        self.lock().client_labels.get(channel).cloned()
    }
}

/// Apply the persisted L1 settings to a new reader, before the runtime
/// accepts clients (so before the first speech): the engine (unless
/// `apply_engine` is false: the command line chose one), the voice, the
/// rate and the volume, each the user's value or else the schema default.
/// A user's value the reader refuses (a voice this engine lacks, such as
/// a Kokoro voice from the Python plugin while only OneCore is installed)
/// is reported and left in `config.json`, so it applies once it is
/// available; the reader keeps its default meanwhile. A default the
/// engine lacks (`af_sarah` on OneCore) is skipped silently.
pub fn apply_reader(
    store: &Store,
    reader: &sonara_reader::ReaderHandle,
    apply_engine: bool,
) -> Vec<String> {
    use sonara_reader::{Key, Value as V};
    let mut problems = Vec::new();
    let mut keys = vec![Key::Voice, Key::Rate, Key::Volume];
    if apply_engine {
        keys.insert(0, Key::Engine);
    }
    for key in keys {
        let user = store.user(key.as_str());
        let from_user = user.is_some();
        let Some(v) = user.or_else(|| {
            (key != Key::Engine)
                .then(|| default(key.as_str()))
                .flatten()
        }) else {
            continue;
        };
        let value = match &v {
            Value::Null => V::Null,
            Value::String(s) => V::Text(s.clone()),
            other => match other.as_u64() {
                Some(n) => V::Number(n),
                None => continue,
            },
        };
        if let Err(e) = reader.set(key, value) {
            if !from_user {
                continue;
            }
            let engine = match reader.get(Key::Engine) {
                Ok(V::Text(t)) => t,
                _ => String::new(),
            };
            problems.push(format!(
                "config.json: {} {v} not applied with engine '{engine}' ({e}); \
                 using the default meanwhile (the setting is kept)",
                key.as_str()
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "sonarad-config-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn every_setting_has_a_valid_default() {
        for s in SCHEMA {
            let d = default(s.key).unwrap();
            assert_eq!(validate(s.key, &d).unwrap(), d, "{}", s.key);
        }
        assert_eq!(default("rate"), Some(json!(250)));
        assert_eq!(default("nope"), None);
    }

    #[test]
    fn the_defaults_are_the_product_defaults() {
        // #202: the maintainer's Python settings, except the mute level.
        for (key, v) in [
            ("voice", json!("af_sarah")),
            ("rate", json!(250)),
            ("volume", json!(100)),
            ("mute_level", json!(0)),
            ("verbosity", json!("skip_code")),
            ("minqueue", json!(5)),
            ("background_policy", json!("all")),
            ("audio_mode", json!("pause")),
            ("duck_level", json!(30)),
            ("debug_log", json!(true)),
        ] {
            assert_eq!(default(key), Some(v), "{key}");
        }
        assert_eq!(default("summaries").unwrap()["enabled"], json!(false));
    }

    #[test]
    fn the_reader_starts_with_the_defaults_and_skips_a_default_voice_it_lacks() {
        use sonara_reader::{Config, Key, ReaderHandle, Registry, Value as V};
        let mut registry = Registry::default();
        registry
            .register(Arc::new(sonara_engine::fake::FakeEngine::new()))
            .unwrap();
        let (out, rx) = sonara_audio::TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap();
        let (store, _) = Store::load(&tmp());
        let problems = apply_reader(&store, &reader, true);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(reader.get(Key::Rate).unwrap(), V::Number(250));
        assert_eq!(
            reader.get(Key::Voice).unwrap(),
            V::Null,
            "the fake has no af_sarah"
        );
        assert!(store.user_keys().is_empty(), "defaults are not the user's");
        reader.shutdown();
    }

    #[test]
    fn validation_refuses_out_of_range_and_wrong_types() {
        for (key, v) in [
            ("volume", json!(101)),
            ("volume", json!(-1)),
            ("volume", json!("50")),
            ("rate", json!(99)),
            ("rate", json!(401)),
            ("voice", json!("")),
            ("voice", json!(3)),
            ("engine", json!(null)),
            ("audio_mode", json!("loud")),
            ("duck_level", json!(1.5)),
            ("mute_level", json!(3)),
            ("verbosity", json!("loud")),
            ("minqueue", json!(11)),
            ("background_policy", json!("silent")),
            ("channel_announce", json!(true)),
            ("debug_log", json!("on")),
            ("summaries", json!({"timeout": 5})),
            ("summaries", json!({"style": "long"})),
            ("summaries", json!({"prompts": {"poem": "x"}})),
            ("summaries", json!(1)),
        ] {
            assert!(validate(key, &v).is_err(), "{key} {v}");
        }
        assert_eq!(validate("rate", &json!(250.0)).unwrap(), json!(250));
        // #214: the verbosity levels before it load as their new names.
        for (old, now) in [
            ("medium", "skip_code"),
            ("quiet", "skip_code"),
            ("all", "everything"),
            ("everything", "everything"),
        ] {
            assert_eq!(
                validate("verbosity", &json!(old)).unwrap(),
                json!(now),
                "{old}"
            );
        }
        assert_eq!(validate("voice", &json!(null)).unwrap(), Value::Null);
        assert_eq!(
            validate(
                "summaries",
                &json!({"model": " sonnet ", "extra": 1, "prompt": "x"})
            )
            .unwrap(),
            json!({"model": "sonnet"}),
            "unknown fields and the derived prompt are dropped"
        );
    }

    #[test]
    fn only_the_keys_the_user_set_are_written() {
        let dir = tmp();
        let (store, problems) = Store::load(&dir);
        assert!(problems.is_empty(), "{problems:?}");
        assert!(
            !dir.join(CONFIG_FILE).exists(),
            "nothing set, nothing written"
        );
        assert_eq!(store.value("rate"), json!(250));
        store.record("rate", &json!(250));
        store.record("volume", &json!(100));
        assert_eq!(
            read(&dir.join(CONFIG_FILE)),
            json!({"rate": 250, "volume": 100}),
            "a key set to its default is the user's choice too"
        );
        store.record("rate", &json!(9999));
        assert_eq!(read(&dir.join(CONFIG_FILE))["rate"], 250, "refused");
        store.forget("volume");
        assert_eq!(read(&dir.join(CONFIG_FILE)), json!({"rate": 250}));
        let (again, _) = Store::load(&dir);
        assert_eq!(again.value("rate"), json!(250));
        assert_eq!(again.value("volume"), json!(100));
    }

    #[test]
    fn summaries_merge_field_by_field_and_keep_the_prompts() {
        let dir = tmp();
        let (store, _) = Store::load(&dir);
        store.record_summaries(json!({"style": "brief"}).as_object().unwrap());
        store.record_summaries(
            json!({"timeout": 30, "prompts": {"brief": "Short."}})
                .as_object()
                .unwrap(),
        );
        assert_eq!(
            read(&dir.join(CONFIG_FILE))["summaries"],
            json!({"style": "brief", "timeout": 30, "prompts": {"brief": "Short."}})
        );
        assert_eq!(
            store.prompts().get("brief").map(String::as_str),
            Some("Short.")
        );
        store.record_summaries(json!({"prompts": {}}).as_object().unwrap());
        assert_eq!(
            read(&dir.join(CONFIG_FILE))["summaries"],
            json!({"style": "brief", "timeout": 30})
        );
    }

    #[test]
    fn a_corrupt_file_or_bad_value_gives_the_defaults_and_a_problem() {
        let dir = tmp();
        std::fs::write(dir.join(CONFIG_FILE), "{not json").unwrap();
        let (store, problems) = Store::load(&dir);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(store.value("volume"), json!(100));
        std::fs::write(
            dir.join(CONFIG_FILE),
            "\u{feff}{\"volume\": 400, \"rate\": 300, \"bogus\": 1, \"_migrated\": {\"from\": \"x\"}}",
        )
        .unwrap();
        let (store, problems) = Store::load(&dir);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(store.value("volume"), json!(100));
        assert_eq!(store.value("rate"), json!(300));
        store.record("rate", &json!(310));
        let saved = read(&dir.join(CONFIG_FILE));
        assert_eq!(saved["_migrated"]["from"], "x", "the marker is kept");
        assert_eq!(saved["bogus"], json!(1), "an unknown key is written back");
        assert_eq!(saved["volume"], json!(400), "a refused value is kept");
        assert_eq!(saved["rate"], json!(310));
        assert_eq!(
            std::fs::read_to_string(dir.join("config.json.bad")).unwrap(),
            "{not json",
            "the unreadable file was copied first"
        );
        store.record("volume", &json!(50));
        assert_eq!(read(&dir.join(CONFIG_FILE))["volume"], json!(50));
    }

    #[test]
    fn prefs_are_capped_cleared_and_read_from_the_python_names() {
        let dir = tmp();
        std::fs::write(
            dir.join(PREFS_FILE),
            r#"{"s1": {"name": "Build", "muted": true, "voice": "af_bella"}, "s2": {}, "": {"name": "x"}}"#,
        )
        .unwrap();
        let (store, _) = Store::load(&dir);
        assert_eq!(
            store.prefs("s1"),
            Prefs {
                label: Some("Build".into()),
                voice: Some("af_bella".into()),
                muted: true
            }
        );
        assert_eq!(store.all_prefs().len(), 1);
        let long = "x".repeat(100);
        let p = store.set_prefs(
            "s1",
            PrefsUpdate {
                label: Some(Some(long)),
                voice: Some(None),
                muted: Some(false),
            },
        );
        assert_eq!(p.label.unwrap().chars().count(), LABEL_MAX);
        assert_eq!(p.voice, None);
        store.set_prefs(
            "s1",
            PrefsUpdate {
                label: Some(Some("  ".into())),
                ..Default::default()
            },
        );
        assert!(store.all_prefs().is_empty(), "an empty entry is dropped");
        for i in 0..(PREFS_CAP + 5) {
            store.set_prefs(
                &format!("c{i}"),
                PrefsUpdate {
                    muted: Some(true),
                    ..Default::default()
                },
            );
        }
        let all = store.all_prefs();
        assert_eq!(all.len(), PREFS_CAP);
        assert_eq!(all[0].0, "c5", "the oldest go first");
        let saved = read(&dir.join(PREFS_FILE));
        assert_eq!(saved.as_object().unwrap().len(), PREFS_CAP);
        assert_eq!(saved["c9"]["muted"], true);
        let (again, _) = Store::load(&dir);
        assert_eq!(again.all_prefs()[0].0, "c5", "the order survives a reload");
    }

    #[test]
    fn the_reader_starts_with_the_persisted_settings_and_keeps_an_unknown_voice() {
        use sonara_reader::{Config, Key, ReaderHandle, Registry, Value as V};
        let mut registry = Registry::default();
        registry
            .register(Arc::new(sonara_engine::fake::FakeEngine::new()))
            .unwrap();
        let (out, rx) = sonara_audio::TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap();
        let dir = tmp();
        std::fs::write(
            dir.join(CONFIG_FILE),
            r#"{"rate": 260, "volume": 40, "voice": "af_sarah", "engine": "fake"}"#,
        )
        .unwrap();
        let (store, _) = Store::load(&dir);
        let problems = apply_reader(&store, &reader, true);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("af_sarah"), "{problems:?}");
        assert_eq!(reader.get(Key::Rate).unwrap(), V::Number(260));
        assert_eq!(reader.get(Key::Volume).unwrap(), V::Number(40));
        assert_eq!(reader.get(Key::Voice).unwrap(), V::Null);
        assert_eq!(store.value("voice"), json!("af_sarah"), "kept for later");
        std::fs::write(dir.join(CONFIG_FILE), r#"{"voice": "silence"}"#).unwrap();
        let (store, _) = Store::load(&dir);
        assert!(apply_reader(&store, &reader, false).is_empty());
        assert_eq!(reader.get(Key::Voice).unwrap(), V::Text("silence".into()));
        reader.shutdown();
    }

    #[test]
    fn the_memory_store_writes_nothing() {
        let store = Store::memory();
        store.record("rate", &json!(300));
        assert_eq!(store.value("rate"), json!(300));
        assert!(store.config_path().is_none());
    }
}
