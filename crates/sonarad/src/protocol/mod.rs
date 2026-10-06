//! Protocol v1 core (spec section 4.1): one request in, one reply out.
//!
//! Synchronous and transport-free: `Server::handle` takes a parsed request
//! and the connection's `Session`, calls the reader facade, and returns the
//! reply plus what the transport must do next (close, start an event
//! stream, end the process). Unknown fields are ignored everywhere.
use crate::agent_ext;
use crate::channels_ext::{self, Slot};
use crate::config::{self, Store};
use crate::engines::Engines;
use crate::engines_ext;
use crate::events::{self, EngineName, EventSet, WireEvent};
use crate::lifetime::{ExitReason, Lifetime};
use crate::quiet::Quiet;
use crate::system_ext::{self, HotkeyTarget, SystemExt, SystemHold, SystemHost};
use crate::trace_log::{self, Origins};
use crate::wire::{self, Code, Failure};
use serde_json::{json, Map, Value};
use sonara_agent::Agent;
use sonara_channels::Channels;
use sonara_reader::{Control, Key, QueueMode, ReaderHandle};
use sonara_system::LogFn;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;

mod extensions;
mod messages;
mod profiles;
#[cfg(test)]
mod tests;

pub const PROTOCOL_MAJOR: u64 = 1;
pub const PROTOCOL_MINOR: u64 = 5;

/// What this host offers (`hello.capabilities`, `runtime.json`): `core` plus
/// each core message type and event stream, so a later minor can add one
/// and a client can `require` it. 1.1 added `engine_status` (readiness and
/// model download progress in `state.engine_status`). 1.2 added `engines`
/// (external engine profiles, `ENGINES_CAPABILITY`), offered only by a host
/// that allows external engines (`Server::capabilities`).
pub const CAPABILITIES: &[&str] = &[
    "core",
    "speak",
    "control",
    "set",
    "get",
    "voices",
    "subscribe",
    "events.state",
    "events.items",
    "events.log",
    "engine_status",
];

/// The capability of the `engine_*` messages (protocol 1.2).
pub const ENGINES_CAPABILITY: &str = "engines";

/// The core messages of external engine profiles (spec 10.2).
pub const ENGINE_TYPES: &[&str] = &[
    "engine_list",
    "engine_add",
    "engine_remove",
    "engine_key",
    "engine_test",
    "engine_reload",
    "engine_models",
];

/// Extensions this host implements. `system` is offered only by a server
/// built `with_system` (see `Server::offered`).
pub const EXTENSIONS: &[&str] = &[channels_ext::NAME, agent_ext::NAME, system_ext::NAME];

/// Message types of the extensions (spec 4.2 to 4.4), from each
/// extension module. They are known, so a client gets `E_UNSUPPORTED`
/// (extension not enabled) rather than `E_UNKNOWN_TYPE`.
const EXTENSION_TYPES: &[&[&str]] = &[channels_ext::TYPES, agent_ext::TYPES, system_ext::TYPES];

/// Extension keys of `set`/`get` and actions of `control`.
const EXTENSION_KEYS: &[&str] = &[
    channels_ext::ANNOUNCE_KEY,
    channels_ext::PREFS_KEY,
    "mute_level",
    "verbosity",
    "read_mode",
    "flush_scope",
    "minqueue",
    "background_policy",
    "summaries",
    "earcons",
    "audio_mode",
    "duck_level",
    "hotkeys",
    "settings_url",
    "runtime",
];
const EXTENSION_ACTIONS: &[&str] = &["next_channel", "flush"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Http,
}

/// Per-connection state. HTTP requests are authenticated by their bearer
/// token, so an HTTP session starts authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    pub transport: Transport,
    pub authed: bool,
    /// A `hello` on this connection asked for the `system` extension: the
    /// TCP transport holds it armed while the connection lives.
    pub system: bool,
}

impl Session {
    pub fn tcp() -> Self {
        Session {
            transport: Transport::Tcp,
            authed: false,
            system: false,
        }
    }

    pub fn http() -> Self {
        Session {
            transport: Transport::Http,
            authed: true,
            system: false,
        }
    }
}

/// What the transport does after sending the reply.
pub enum After {
    Nothing,
    /// Close the connection (failed authentication).
    Close,
    /// Replace this connection's event stream with this one.
    Subscribe(mpsc::Receiver<WireEvent>),
    /// End the process (an accepted takeover, or `shutdown`).
    Exit,
}

pub struct Outcome {
    pub reply: Value,
    /// The error code when the reply is an error (for HTTP statuses).
    pub code: Option<Code>,
    pub after: After,
}

pub struct Server {
    reader: ReaderHandle,
    token: String,
    engine: EngineName,
    lifetime: Arc<Lifetime>,
    /// Set once a takeover is accepted or the idle exit decided. `speak`
    /// and `control` hold this lock while they reach the reader, so the
    /// idle check and setting it are atomic with respect to them: nothing
    /// is accepted after the decision and then silently dropped by the
    /// exit (#194).
    retiring: Arc<Mutex<bool>>,
    /// The `channels` extension, once a client enabled it (it stays
    /// enabled for the life of the process).
    channels: Slot,
    /// The `agent` extension (needs `channels`), once a client enabled it.
    agent: agent_ext::Slot,
    /// The earcon clips the agent plays (bundled, or a custom folder in
    /// front: `with_earcons`).
    earcons: Arc<sonara_agent::Library>,
    /// The `system` extension, when this host offers it (`with_system`).
    system: Option<Arc<SystemExt>>,
    /// Serializes enabling an extension.
    enabling: Mutex<()>,
    /// The persisted settings (`config.json`, `session_prefs.json`).
    store: Arc<Store>,
    /// Serializes `set`: the change and its record in `config.json`, so two
    /// clients setting one key leave the file holding the value in force.
    setting: Mutex<()>,
    /// The support log's activity lines (`with_log`; else stderr).
    log: Option<LogFn>,
    /// What produced each channel entry the agent added (#219), for the
    /// reading log's `read text` lines.
    origins: Origins,
    /// External engine profiles, when this host allows them
    /// (`with_engines`; without, `engine_*` is `E_UNSUPPORTED`).
    engines: Option<Arc<Engines>>,
    /// Muted: external engines send nothing (#227, `crate::quiet`).
    quiet: Quiet,
}

pub(crate) type Handled = Result<(Map<String, Value>, After), Failure>;

pub(crate) fn bad(message: impl Into<String>) -> Failure {
    Failure::new(Code::BadRequest, message)
}

pub(crate) fn reader_failure(e: sonara_reader::Error) -> Failure {
    use sonara_engine::Error as E;
    use sonara_reader::Error as R;
    let code = match &e {
        R::BadValue { .. } => Code::BadRequest,
        // The reader shut down: the runtime is exiting (#194).
        R::Closed => Code::Busy,
        R::Engine(E::UnknownEngine(_)) | R::Engine(E::UnknownVoice(_)) => Code::NotFound,
        _ => Code::Engine,
    };
    Failure::new(code, e.to_string())
}

/// Constant-time comparison, so the token cannot be guessed byte by byte.
pub fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

pub(crate) fn opt_str<'a>(
    m: &'a Map<String, Value>,
    field: &str,
) -> Result<Option<&'a str>, Failure> {
    match m.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(bad(format!("'{field}' must be a string"))),
    }
}

fn opt_bool(m: &Map<String, Value>, field: &str) -> Result<bool, Failure> {
    match m.get(field) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(bad(format!("'{field}' must be true or false"))),
    }
}

fn str_list<'a>(m: &'a Map<String, Value>, field: &str) -> Result<Option<Vec<&'a str>>, Failure> {
    match m.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| bad(format!("'{field}' must be a list of strings")))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(bad(format!("'{field}' must be a list of strings"))),
    }
}

fn parse_control(action: &str) -> Option<Control> {
    Some(match action {
        "play" => Control::Play,
        "pause" => Control::Pause,
        "toggle" => Control::Toggle,
        "stop" => Control::Stop,
        "skip" => Control::Skip,
        "previous" => Control::Previous,
        "next" => Control::Next,
        "restart" => Control::Restart,
        "mute" => Control::Mute,
        "unmute" => Control::Unmute,
        _ => return None,
    })
}

impl Server {
    pub fn new(reader: ReaderHandle, token: String, lifetime: Arc<Lifetime>) -> Self {
        let engine = match reader.get(Key::Engine) {
            Ok(sonara_reader::Value::Text(t)) => t,
            _ => String::new(),
        };
        let agent: agent_ext::Slot = Arc::new(OnceLock::new());
        let quiet = Quiet::new(reader.clone(), agent.clone());
        Server {
            reader,
            token,
            engine: Arc::new(Mutex::new(engine)),
            lifetime,
            retiring: Arc::new(Mutex::new(false)),
            channels: Arc::new(OnceLock::new()),
            agent,
            earcons: Arc::new(sonara_agent::Library::bundled()),
            system: None,
            enabling: Mutex::new(()),
            store: Store::memory(),
            setting: Mutex::new(()),
            log: None,
            origins: Origins::default(),
            engines: None,
            quiet,
        }
    }

    /// Allow external engine profiles (capability `engines`).
    pub fn with_engines(mut self, engines: Arc<Engines>) -> Self {
        self.quiet.attach(engines.hold().clone());
        self.engines = Some(engines);
        self
    }

    pub fn engines(&self) -> Option<&Arc<Engines>> {
        self.engines.as_ref()
    }

    /// `hello.capabilities` and `runtime.json`: the core ones, plus
    /// `engines` when this host allows external engines.
    pub fn capabilities(&self) -> Vec<&'static str> {
        let mut c = CAPABILITIES.to_vec();
        if self.engines.is_some() {
            c.push(ENGINES_CAPABILITY);
        }
        c
    }

    fn current_engine(&self) -> String {
        events::engine_name(&self.engine)
    }

    /// The origins of channel entries (shared with the reading log).
    pub fn origins(&self) -> Origins {
        self.origins.clone()
    }

    /// A troubleshooting line (#219): to the log only, never stderr.
    fn trace(&self, line: &str) {
        if let Some(log) = &self.log {
            log(line);
        }
    }

    /// Write the activity lines (decisions, hotkeys, other apps' audio) to
    /// `log` (`logs\sonarad.log`). Call it before `with_system`.
    pub fn with_log(mut self, log: LogFn) -> Self {
        debug_assert!(self.system.is_none(), "with_log before with_system");
        self.log = Some(log);
        self
    }

    fn note(&self, line: &str) {
        sonara_system::log::emit(self.log.as_ref(), line);
    }

    /// Persist settings in `store` (default: in memory only). Call it
    /// before `with_system`, which applies the stored audio settings.
    pub fn with_config(mut self, store: Arc<Store>) -> Self {
        debug_assert!(self.system.is_none(), "with_config before with_system");
        self.store = store;
        self
    }

    /// Play `earcons` (custom WAVs in front of the bundled clips) once the
    /// agent extension is enabled.
    pub fn with_earcons(mut self, earcons: Arc<sonara_agent::Library>) -> Self {
        self.earcons = earcons;
        self
    }

    /// The persisted settings.
    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// Offer the `system` extension on this platform and home.
    pub fn with_system(mut self, host: SystemHost) -> Self {
        let target = HotkeyTarget {
            reader: self.reader.clone(),
            channels: self.channels.clone(),
            agent: self.agent.clone(),
            retiring: self.retiring.clone(),
            store: self.store.clone(),
            cues: None,
            log: self.log.clone(),
            quiet: self.quiet.clone(),
        };
        self.system = Some(Arc::new(SystemExt::new(host, target)));
        self
    }

    /// The `system` extension, if this host offers it (enabled or not).
    pub fn system(&self) -> Option<&Arc<SystemExt>> {
        self.system.as_ref()
    }

    /// Hold `system` armed for a connection whose `hello` asked for it.
    pub fn hold_system(&self) -> Option<SystemHold> {
        self.system.as_ref().map(|s| s.hold())
    }

    /// The extensions this host offers (`runtime.json`, `require`).
    pub fn offered(&self) -> Vec<&'static str> {
        EXTENSIONS
            .iter()
            .copied()
            .filter(|e| *e != system_ext::NAME || self.system.is_some())
            .collect()
    }

    /// Whether Sonara is muted and external engines held (#227).
    pub fn quiet(&self) -> &Quiet {
        &self.quiet
    }

    pub fn reader(&self) -> &ReaderHandle {
        &self.reader
    }

    pub fn lifetime(&self) -> &Arc<Lifetime> {
        &self.lifetime
    }

    pub fn token_ok(&self, token: &str) -> bool {
        token_eq(token, &self.token)
    }

    /// Nothing playing and nothing queued (a paused item is not idle), and
    /// no channel with text it would read.
    pub fn is_idle(&self) -> bool {
        let reader = match self.reader.state() {
            Ok(s) => s.now_playing.is_none() && s.queued == 0,
            Err(_) => true,
        };
        reader && self.channels.get().is_none_or(Channels::is_idle)
    }

    /// Something is being read right now (an item playing, not paused): the
    /// idle exit waits for it. A paused item does not hold the process.
    pub fn is_reading(&self) -> bool {
        match self.reader.state() {
            Ok(s) => s.now_playing.is_some() && !s.paused,
            Err(_) => false,
        }
    }

    /// The idle exit (spec section 3), decided under the admission lock:
    /// true (and nothing is admitted from now on) when no client is
    /// connected, the idle time ran out since the last activity and
    /// nothing is being read; false otherwise. A request that touched the
    /// lifetime before this check keeps the runtime; one admitted after it
    /// gets `E_BUSY` instead of being accepted and then lost (#194).
    pub fn retire_if_idle(&self) -> bool {
        let mut retiring = self.retiring.lock().unwrap_or_else(|p| p.into_inner());
        if *retiring {
            return true;
        }
        let life = &self.lifetime;
        if !life.may_idle_exit() || !life.idle_expired() || self.is_reading() {
            return false;
        }
        *retiring = true;
        true
    }

    /// Hold the admission lock for a request that could start speech, or
    /// `E_BUSY` once a takeover was accepted or the idle exit decided.
    fn admit(&self) -> Result<std::sync::MutexGuard<'_, bool>, Failure> {
        let retiring = self.retiring.lock().unwrap_or_else(|p| p.into_inner());
        if *retiring {
            return Err(Failure::new(Code::Busy, "this runtime is exiting"));
        }
        Ok(retiring)
    }

    /// Start an event stream (used by `subscribe` and `GET /v1/events`).
    pub fn events(&self, set: EventSet) -> Result<mpsc::Receiver<WireEvent>, Failure> {
        events::subscribe(
            &self.reader,
            self.engine.clone(),
            self.channels.clone(),
            self.agent.clone(),
            self.system.clone().filter(|s| s.is_enabled()),
            set,
        )
        .map_err(reader_failure)
    }

    /// Handle one request. A request that is not a JSON object, or has no
    /// `type`, is `E_BAD_REQUEST` (or `E_AUTH` before `hello` on TCP).
    pub fn handle(&self, session: &mut Session, request: &Value) -> Outcome {
        let empty = Map::new();
        let (m, id) = match request {
            Value::Object(m) => (m, m.get("id")),
            _ => (&empty, None),
        };
        let kind = m.get("type").and_then(Value::as_str);
        let over_http = session.transport == Transport::Http;
        // Nothing a connection sends before `hello` is logged (the token
        // is stripped from the `hello` itself).
        if request.is_object() && (session.authed || kind == Some("hello")) {
            if let Some(line) = trace_log::input_line(m, over_http, trace_log::debug()) {
                self.trace(&line);
            }
        }
        let result = if !session.authed && kind != Some("hello") {
            Err(Failure::new(
                Code::Auth,
                "the first message must be hello with the token",
            ))
        } else if !request.is_object() {
            Err(bad("a request is a JSON object"))
        } else {
            match kind {
                None => Err(bad("missing 'type'")),
                Some(k) => self.dispatch(session, k, m),
            }
        };
        match result {
            Ok((fields, after)) => Outcome {
                reply: wire::ok_reply(id, fields),
                code: None,
                after,
            },
            Err(f) => {
                let k = kind.unwrap_or("");
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let allowed = session.authed || trace_log::unauthed_allowed(now);
                if allowed && trace_log::logged(k, over_http) {
                    self.trace(&trace_log::failed_line(k, f.code.as_str(), &f.message));
                }
                let after = if f.code == Code::Auth && session.transport == Transport::Tcp {
                    After::Close
                } else {
                    After::Nothing
                };
                Outcome {
                    reply: wire::error_reply(id, &f),
                    code: Some(f.code),
                    after,
                }
            }
        }
    }

    fn dispatch(&self, session: &mut Session, kind: &str, m: &Map<String, Value>) -> Handled {
        match kind {
            "hello" => self.hello(session, m),
            "speak" => self.speak(m),
            "control" => self.control(m),
            "set" => self.set(m),
            "get" => self.get(m),
            "voices" => self.voices(m),
            "subscribe" => self.subscribe(session, m),
            k if ENGINE_TYPES.contains(&k) => self.engine_message(k, m),
            k if channels_ext::TYPES.contains(&k) && self.channels.get().is_some() => {
                let ch = self.channels.get().expect("checked");
                let _admitted = self.admit()?;
                match (kind, self.agent.get()) {
                    ("channel_open", _) => channels_ext::open(ch, &self.store, m),
                    ("channel_close", Some(a)) => agent_ext::close(a, m),
                    ("channel_close", None) => channels_ext::close(ch, m),
                    _ => channels_ext::focus(ch, m),
                }
            }
            k if agent_ext::TYPES.contains(&k) && self.agent.get().is_some() => {
                let a = self.agent.get().expect("checked");
                let _admitted = self.admit()?;
                // `label` names the message's channel; `earcon` has none.
                if let Some(id) = m.get("channel").and_then(Value::as_str) {
                    let label = opt_str(m, "label")?;
                    channels_ext::apply_label(a.channels(), &self.store, id, label);
                }
                match k {
                    "stream" => agent_ext::stream(a, m),
                    "turn_start" => agent_ext::turn_start(a, m),
                    "turn_end" => agent_ext::turn_end(a, m),
                    "ask" => {
                        let r = agent_ext::ask(a, m);
                        if r.is_ok() {
                            self.note(&agent_ext::ask_line(a, m));
                        }
                        r
                    }
                    "earcon" => agent_ext::earcon(a, m),
                    "tool" => agent_ext::tool(a, m),
                    _ => agent_ext::answered(a, m),
                }
            }
            k if system_ext::TYPES.contains(&k)
                && self.system.as_ref().is_some_and(|s| s.is_enabled()) =>
            {
                let s = self.system.as_ref().expect("checked");
                match k {
                    "shutdown" => self.shutdown(),
                    _ => s.preview(&self.reader, m),
                }
            }
            k if EXTENSION_TYPES.iter().any(|t| t.contains(&k)) => Err(Failure::new(
                Code::Unsupported,
                format!("'{k}' belongs to an extension this host does not offer"),
            )),
            k => Err(Failure::new(
                Code::UnknownType,
                format!("unknown message type '{k}'"),
            )),
        }
    }

    /// Carry out an accepted takeover or a `shutdown`: called by the
    /// transport after the reply went out.
    pub fn exit_for_takeover(&self) {
        self.lifetime.request_exit(ExitReason::Takeover);
    }
}
