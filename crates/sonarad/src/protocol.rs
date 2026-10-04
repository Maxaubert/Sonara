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

pub const PROTOCOL_MAJOR: u64 = 1;
pub const PROTOCOL_MINOR: u64 = 4;

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
];

/// Extensions this host implements. `system` is offered only by a server
/// built `with_system` (see `Server::offered`).
pub const EXTENSIONS: &[&str] = &[channels_ext::NAME, agent_ext::NAME, system_ext::NAME];

/// Message types of the extensions (spec 4.2 to 4.4). They are known, so a
/// client gets `E_UNSUPPORTED` (extension not enabled) rather than
/// `E_UNKNOWN_TYPE`.
const EXTENSION_TYPES: &[&str] = &[
    "channel_open",
    "channel_close",
    "focus",
    "stream",
    "turn_start",
    "turn_end",
    "ask",
    "earcon",
    "tool",
    "answered",
    "preview",
    "shutdown",
];

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

    /// The `channels` extension, if a client enabled it.
    pub fn channels(&self) -> Option<&Channels> {
        self.channels.get()
    }

    /// The `agent` extension, if a client enabled it.
    pub fn agent(&self) -> Option<&Agent> {
        self.agent.get()
    }

    /// Enable an extension this host offers (idempotent). `agent` needs
    /// `channels` and enables it first. The layer starts with the persisted
    /// settings.
    fn enable(&self, name: &str) -> Result<(), Failure> {
        let _one = self.enabling.lock().unwrap_or_else(|p| p.into_inner());
        let engine = |e: String| Failure::new(Code::Engine, e);
        if (name == channels_ext::NAME || name == agent_ext::NAME) && self.channels.get().is_none()
        {
            let config = sonara_channels::Config {
                announce: self.store.value(channels_ext::ANNOUNCE_KEY) != "off",
                ..Default::default()
            };
            let ch =
                Channels::new(self.reader.clone(), config).map_err(|e| engine(e.to_string()))?;
            // The channels the user muted stay muted (#196).
            channels_ext::apply_mutes(&ch, &self.store);
            if let Some(log) = self.log.clone() {
                let origins = self.origins.clone();
                ch.on_drop(Some(Arc::new(move |d: &sonara_channels::Dropped| {
                    let origin = origins.get(d.entry);
                    log(&trace_log::drop_line(
                        d,
                        origin.as_ref(),
                        trace_log::debug(),
                    ));
                })));
            }
            let _ = self.channels.set(ch);
        }
        if name == agent_ext::NAME && self.agent.get().is_none() {
            let ch = self.channels.get().expect("enabled above").clone();
            let config = sonara_agent::Config {
                settings: agent_ext::settings_from(&self.store),
                earcons: self.earcons.clone(),
                ..Default::default()
            };
            let agent = match Agent::new(ch.clone(), config) {
                Ok(a) => a,
                // A runtime without the summarizer: start with summaries off.
                Err(sonara_agent::Error::NoSummarizer) => {
                    let mut settings = agent_ext::settings_from(&self.store);
                    settings.summaries.enabled = false;
                    let config = sonara_agent::Config {
                        settings,
                        earcons: self.earcons.clone(),
                        ..Default::default()
                    };
                    Agent::new(ch, config).map_err(|e| engine(e.to_string()))?
                }
                Err(e) => return Err(engine(e.to_string())),
            };
            if let Some(log) = self.log.clone() {
                let origins = self.origins.clone();
                agent.on_trace(Some(Arc::new(move |t: &sonara_agent::Trace| {
                    if let sonara_agent::Traced::Spoken { kind, entry, .. } = &t.what {
                        origins.record(*entry, kind, &t.source);
                    }
                    log(&trace_log::agent_line(t, trace_log::debug()));
                })));
            }
            let _ = self.agent.set(agent);
            // A saved mute level applies from the start (#227).
            self.quiet.sync();
        }
        if name == system_ext::NAME {
            if let Some(s) = &self.system {
                s.enable(&self.reader)?;
            }
        }
        Ok(())
    }

    /// The extensions enabled now.
    pub fn enabled(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.channels.get().is_some() {
            v.push(channels_ext::NAME);
        }
        if self.agent.get().is_some() {
            v.push(agent_ext::NAME);
        }
        if self.system.as_ref().is_some_and(|s| s.is_enabled()) {
            v.push(system_ext::NAME);
        }
        v
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
            "channel_open" | "channel_close" | "focus" if self.channels.get().is_some() => {
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
                if let Some(id) = m.get("channel").and_then(Value::as_str) {
                    channels_ext::apply_label(a.channels(), &self.store, id);
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
            k if EXTENSION_TYPES.contains(&k) => Err(Failure::new(
                Code::Unsupported,
                format!("'{k}' belongs to an extension this host does not offer"),
            )),
            k => Err(Failure::new(
                Code::UnknownType,
                format!("unknown message type '{k}'"),
            )),
        }
    }

    fn hello_fields(&self, unavailable: Vec<&str>) -> Map<String, Value> {
        let mut f = Map::new();
        f.insert("version".into(), json!(crate::VERSION));
        f.insert(
            "protocol".into(),
            json!({"major": PROTOCOL_MAJOR, "minor": PROTOCOL_MINOR}),
        );
        f.insert("capabilities".into(), json!(self.capabilities()));
        f.insert("extensions".into(), json!(self.enabled()));
        f.insert("unavailable".into(), json!(unavailable));
        f
    }

    fn hello(&self, session: &mut Session, m: &Map<String, Value>) -> Handled {
        if !session.authed {
            let token = m.get("token").and_then(Value::as_str).unwrap_or("");
            if !self.token_ok(token) {
                return Err(Failure::new(Code::Auth, "wrong or missing token"));
            }
        }
        if opt_bool(m, "takeover")? {
            let mut retiring = self.retiring.lock().unwrap_or_else(|p| p.into_inner());
            if !*retiring && !self.is_idle() {
                return Err(Failure::new(
                    Code::Busy,
                    "something is playing or queued; retry after the current item",
                ));
            }
            *retiring = true;
            let mut f = self.hello_fields(Vec::new());
            f.insert("takeover".into(), Value::Bool(true));
            return Ok((f, After::Exit));
        }
        if let Some(p) = m.get("protocol").filter(|p| !p.is_null()) {
            let major = p.get("major").and_then(Value::as_u64);
            match major {
                Some(PROTOCOL_MAJOR) => {}
                Some(other) => {
                    return Err(Failure::new(
                        Code::Incompatible,
                        format!(
                            "this host speaks protocol {PROTOCOL_MAJOR}.{PROTOCOL_MINOR}, \
                             not {other}.x"
                        ),
                    ))
                }
                None => return Err(bad("'protocol.major' must be a number")),
            }
        }
        let offered = self.offered();
        let capabilities = self.capabilities();
        let require = str_list(m, "require")?.unwrap_or_default();
        let missing: Vec<&str> = require
            .iter()
            .copied()
            .filter(|r| !capabilities.contains(r) && !offered.contains(r))
            .collect();
        if !missing.is_empty() {
            return Err(Failure::new(
                Code::Unsupported,
                format!("this host does not offer: {}", missing.join(", ")),
            ));
        }
        let wanted = str_list(m, "extensions")?.unwrap_or_default();
        let unavailable: Vec<&str> = wanted
            .iter()
            .copied()
            .filter(|e| !offered.contains(e))
            .collect();
        // An extension is enabled for the instance when any client asks for
        // it (or requires it) and stays enabled (spec section 3).
        for e in wanted.iter().chain(require.iter()) {
            if offered.contains(e) {
                self.enable(e)?;
            }
        }
        let keep_alive = opt_bool(m, "keep_alive")?;
        if keep_alive {
            self.lifetime.set_keep_alive();
        }
        // `system` is armed (ducking, hotkeys) while it is needed: for this
        // TCP connection, or for good with keep_alive.
        let wants_system = wanted
            .iter()
            .chain(require.iter())
            .any(|e| *e == system_ext::NAME);
        if let (true, Some(s)) = (wants_system, &self.system) {
            if keep_alive {
                s.keep();
            }
            if session.transport == Transport::Tcp {
                session.system = true;
            }
        }
        session.authed = true;
        Ok((self.hello_fields(unavailable), After::Nothing))
    }

    fn speak(&self, m: &Map<String, Value>) -> Handled {
        let text = match m.get("text") {
            Some(Value::String(t)) => t,
            _ => return Err(bad("'text' must be a string")),
        };
        let mode = match opt_str(m, "mode")? {
            None => None,
            Some("append") => Some(QueueMode::Append),
            Some("replace") => Some(QueueMode::Replace),
            Some(other) => return Err(bad(format!("unknown mode '{other}'"))),
        };
        let interrupt = opt_bool(m, "interrupt")?;
        let label = opt_str(m, "label")?.map(str::to_string);
        // Without the extension, `channel` is an unknown field (ignored).
        if let Some(ch) = self.channels.get() {
            if let Some(id) = opt_str(m, "channel")? {
                let _admitted = self.admit()?;
                return channels_ext::speak(ch, id, text, mode, interrupt, label);
            }
        }
        let _admitted = self.admit()?;
        let id = self
            .reader
            .speak(text, mode.unwrap_or(QueueMode::Append), interrupt, label)
            .map_err(reader_failure)?;
        let mut f = Map::new();
        f.insert("item_id".into(), json!(id.0));
        Ok((f, After::Nothing))
    }

    /// `control flush` (#228): the flush hotkey. It stops the session
    /// being read; with the agent the rest of that reply is skipped, and
    /// with `flush_scope` `all` every other session's ready messages go
    /// too.
    fn flush(&self, ch: &Channels, m: &Map<String, Value>) -> Handled {
        if opt_str(m, "channel")?.is_some() {
            return Err(bad(
                "'flush' acts on the session being read; use 'stop' with a channel",
            ));
        }
        let _admitted = self.admit()?;
        let (report, scope) = match self.agent.get() {
            Some(a) => (
                a.flush().map_err(agent_ext::failure)?,
                a.settings().flush_scope,
            ),
            None => (
                sonara_channels::FlushReport {
                    flushed: ch.stop_reading("flush").map_err(channels_ext::failure)?,
                    others: Vec::new(),
                },
                sonara_agent::FlushScope::Session,
            ),
        };
        Ok((channels_ext::flushed_fields(&report, scope), After::Nothing))
    }

    /// `control`. A mute holds external engines before it applies and
    /// `mute`/`unmute` leave them held exactly while Sonara is muted
    /// (#227).
    fn control(&self, m: &Map<String, Value>) -> Handled {
        let action = opt_str(m, "action")?.ok_or_else(|| bad("missing 'action'"))?;
        if action == "mute" {
            self.quiet.hold_now();
        }
        let done = self.control_now(m, action);
        if action == "mute" || action == "unmute" {
            self.quiet.sync();
        }
        done
    }

    fn control_now(&self, m: &Map<String, Value>, action: &str) -> Handled {
        if let Some(ch) = self.channels.get() {
            if action == "flush" {
                return self.flush(ch, m);
            }
            let c = match parse_control(action) {
                Some(c) => Some(c),
                None if action == "next_channel" => None,
                None => return Err(bad(format!("unknown action '{action}'"))),
            };
            let _admitted = self.admit()?;
            // With the agent on, a stop without a channel also drops the
            // summaries cooking and the held decisions.
            if let (Some(a), Some(Control::Stop), None) =
                (self.agent.get(), c, opt_str(m, "channel")?)
            {
                a.stop().map_err(agent_ext::failure)?;
                return Ok((Map::new(), After::Nothing));
            }
            return channels_ext::control(ch, c, m);
        }
        let c = match parse_control(action) {
            Some(c) => c,
            None if EXTENSION_ACTIONS.contains(&action) => {
                return Err(Failure::new(
                    Code::Unsupported,
                    format!("action '{action}' belongs to an extension"),
                ))
            }
            None => return Err(bad(format!("unknown action '{action}'"))),
        };
        let _admitted = self.admit()?;
        self.reader.control(c).map_err(reader_failure)?;
        Ok((Map::new(), After::Nothing))
    }

    fn key(&self, m: &Map<String, Value>) -> Result<Key, Failure> {
        let name = opt_str(m, "key")?.ok_or_else(|| bad("missing 'key'"))?;
        match Key::parse(name) {
            Some(k) => Ok(k),
            None if EXTENSION_KEYS.contains(&name) => Err(Failure::new(
                Code::Unsupported,
                format!("setting '{name}' belongs to an extension"),
            )),
            None => Err(bad(format!("unknown setting '{name}'"))),
        }
    }

    fn key_value(&self, key: Key) -> Handled {
        let value = self.reader.get(key).map_err(reader_failure)?;
        let mut f = Map::new();
        f.insert("key".into(), json!(key.as_str()));
        f.insert("value".into(), wire::setting_to_json(&value));
        Ok((f, After::Nothing))
    }

    /// `set`/`get` of an enabled extension's key, if `m` names one.
    fn extension_setting(&self, m: &Map<String, Value>, set: bool) -> Option<Handled> {
        let name = opt_str(m, "key").ok().flatten()?;
        let value = if set {
            Some(m.get("value").unwrap_or(&Value::Null))
        } else {
            None
        };
        if let (Some(a), true) = (self.agent.get(), agent_ext::KEYS.contains(&name)) {
            return Some(agent_ext::setting(a, &self.store, name, value));
        }
        if let Some(s) = self.system.as_ref().filter(|s| s.is_enabled()) {
            if system_ext::KEYS.contains(&name) {
                let mut handled = s.setting(name, value);
                if name == "runtime" {
                    self.add_engine_status(&mut handled);
                }
                return Some(handled);
            }
        }
        let ch = self.channels.get()?;
        match name {
            channels_ext::ANNOUNCE_KEY => Some(channels_ext::announce(ch, value)),
            channels_ext::PREFS_KEY => Some(channels_ext::prefs_setting(
                ch,
                self.agent.get(),
                &self.store,
                value,
            )),
            _ => None,
        }
    }

    /// `get runtime` also carries `engine_status` (as in `state`, #214), so
    /// a client that polls instead of subscribing sees the engine's
    /// readiness and its model download.
    fn add_engine_status(&self, handled: &mut Handled) {
        let Ok((fields, _)) = handled else {
            return;
        };
        let Ok(status) = self.reader.engine_status() else {
            return;
        };
        let engine = events::engine_name(&self.engine);
        if let Some(Value::Object(v)) = fields.get_mut("value") {
            v.insert(
                "engine_status".into(),
                wire::engine_status_json(&engine, &status),
            );
        }
    }

    /// `set`, persisted once it took effect: the value now in force is
    /// stored for every key of the schema (`summaries` is stored by the
    /// agent extension, field by field; the keymap and channel preferences
    /// have files of their own).
    fn set(&self, m: &Map<String, Value>) -> Handled {
        let _one_at_a_time = self.setting.lock().unwrap_or_else(|p| p.into_inner());
        let engine_before = self
            .engine
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mute_before = self.agent.get().map(|a| a.settings().mute_level);
        let mute_key = opt_str(m, "key").ok().flatten() == Some("mute_level");
        if mute_key
            && m.get("value")
                .and_then(Value::as_u64)
                .is_some_and(|l| l >= 1)
        {
            // Nothing is sent from here on (#227).
            self.quiet.hold_now();
        }
        let result = self.set_now(m);
        if mute_key {
            self.quiet.sync();
        }
        if let (Ok((fields, _)), Some(name)) = (&result, opt_str(m, "key").ok().flatten()) {
            // The mute level's spoken cue (the hotkey's, #197), when it
            // changed; `audio_mode` and `duck_level` speak theirs in the
            // system extension.
            if let (Some(s), "mute_level", Some(before)) = (&self.system, name, mute_before) {
                let now = fields.get("value").and_then(Value::as_u64);
                if let Some(level) = now.filter(|l| *l != u64::from(before)) {
                    s.cue_local(crate::cues::mute_level_cue(level));
                }
            }
            if name != "summaries" && config::setting(name).is_some() {
                if let Some(v) = fields.get("value") {
                    self.store.record(name, v);
                }
                if name == "engine" {
                    self.engine_switched(&engine_before);
                }
            }
        }
        result
    }

    /// After `set engine`: a saved voice the new engine lacks is replaced by
    /// the voice the reader now uses. Picking the same engine again keeps a
    /// saved voice that could not apply (it waits until it is available).
    fn engine_switched(&self, before: &str) {
        let now = self
            .engine
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if now == before {
            return;
        }
        let Some(Value::String(saved)) = self.store.user("voice") else {
            // No voice of the user's: the default voice (af_sarah) comes
            // back with an engine that lists it (an external engine takes
            // any id, so it must list it: its own voice applies otherwise).
            if let Some(Value::String(d)) = config::default("voice") {
                let listed = self
                    .reader
                    .voices(Some(&now))
                    .is_ok_and(|vs| vs.iter().any(|v| v.id == d));
                if listed {
                    let _ = self.reader.set(Key::Voice, sonara_reader::Value::Text(d));
                }
            }
            return;
        };
        let offered = self
            .reader
            .voices(Some(&now))
            .map(|vs| vs.iter().any(|v| v.id == saved || v.name == saved))
            .unwrap_or(false);
        if !offered {
            if let Ok(v) = self.reader.get(Key::Voice) {
                self.store.record("voice", &wire::setting_to_json(&v));
            }
        }
    }

    /// `set`/`get` of `debug_log` (#219), a host setting: text and
    /// payloads in the troubleshooting log, on or off.
    fn debug_setting(&self, m: &Map<String, Value>, set: bool) -> Option<Handled> {
        if opt_str(m, "key").ok().flatten() != Some(trace_log::DEBUG_KEY) {
            return None;
        }
        if set {
            let raw = m.get("value").unwrap_or(&Value::Null);
            match config::validate(trace_log::DEBUG_KEY, raw) {
                Ok(v) => trace_log::set_debug(v.as_bool().unwrap_or(true)),
                Err(e) => return Some(Err(bad(e))),
            }
        }
        let mut f = Map::new();
        f.insert("key".into(), json!(trace_log::DEBUG_KEY));
        f.insert("value".into(), json!(trace_log::debug()));
        Some(Ok((f, After::Nothing)))
    }

    fn set_now(&self, m: &Map<String, Value>) -> Handled {
        if let Some(done) = self.debug_setting(m, true) {
            return done;
        }
        if let Some(done) = self.extension_setting(m, true) {
            return done;
        }
        let key = self.key(m)?;
        let raw = m.get("value").unwrap_or(&Value::Null);
        let value = wire::setting_from_json(raw)
            .ok_or_else(|| bad("'value' must be a non-negative integer, a string or null"))?;
        self.reader.set(key, value).map_err(reader_failure)?;
        if key == Key::Engine {
            if let Ok(sonara_reader::Value::Text(t)) = self.reader.get(Key::Engine) {
                *self.engine.lock().unwrap_or_else(|p| p.into_inner()) = t;
            }
        }
        self.key_value(key)
    }

    fn get(&self, m: &Map<String, Value>) -> Handled {
        if let Some(done) = self.debug_setting(m, false) {
            return done;
        }
        if let Some(done) = self.extension_setting(m, false) {
            return done;
        }
        let key = self.key(m)?;
        self.key_value(key)
    }

    fn voices(&self, m: &Map<String, Value>) -> Handled {
        if m.contains_key("profile") {
            let engines = self.engines.as_ref().ok_or_else(engines_ext::refused)?;
            return engines_ext::draft_voices(engines, m);
        }
        let engine = opt_str(m, "engine")?;
        let refresh = opt_bool(m, "refresh")?;
        if let (Some(e), Some(id)) = (&self.engines, engine) {
            if let Some(done) = engines_ext::voices(e, &self.reader, id, refresh) {
                return done;
            }
        }
        let voices = self.reader.voices(engine).map_err(reader_failure)?;
        let mut f = Map::new();
        f.insert(
            "voices".into(),
            Value::Array(voices.iter().map(wire::voice_json).collect()),
        );
        Ok((f, After::Nothing))
    }

    fn subscribe(&self, session: &Session, m: &Map<String, Value>) -> Handled {
        if session.transport == Transport::Http {
            return Err(bad("over HTTP, subscribe with GET /v1/events?events=..."));
        }
        let set = match str_list(m, "events")? {
            None => EventSet::ALL,
            Some(names) => EventSet::parse(names).map_err(|n| {
                Failure::new(Code::Unsupported, format!("unknown event stream '{n}'"))
            })?,
        };
        if set.earcons && self.agent.get().is_none() {
            return Err(Failure::new(
                Code::Unsupported,
                "the 'earcons' events belong to the agent extension",
            ));
        }
        if set.cues && !self.system.as_ref().is_some_and(|s| s.is_enabled()) {
            return Err(Failure::new(
                Code::Unsupported,
                "the 'cues' events belong to the system extension",
            ));
        }
        let rx = self.events(set)?;
        let mut f = Map::new();
        f.insert("events".into(), json!(set.names()));
        Ok((f, After::Subscribe(rx)))
    }

    /// `engine_list`, `engine_add`, `engine_remove`, `engine_key`,
    /// `engine_test` (spec 10.2), `engine_reload` (protocol 1.3).
    fn engine_message(&self, kind: &str, m: &Map<String, Value>) -> Handled {
        let engines = self.engines.as_ref().ok_or_else(engines_ext::refused)?;
        let current = self.current_engine();
        match kind {
            "engine_list" => engines_ext::list(engines, &current),
            "engine_add" => engines_ext::add(engines, &self.reader, m, &current),
            "engine_key" => engines_ext::key(engines, m),
            // The provider round trip runs outside the admission lock
            // (it can take the profile's whole timeout); only the play is
            // admitted, so speech and controls never wait for a test.
            "engine_test" => engines_ext::test(engines, &self.reader, m, || self.admit()),
            "engine_reload" => self.engine_reload(engines, &current),
            _ => self.engine_remove(engines, m, &current),
        }
    }

    /// `engine_reload` (protocol 1.3, no fields): read `engines.json` again
    /// after the user (or `sonara engines add --kind command`) changed it.
    /// It takes no profile: a `command` engine reaches the runtime only
    /// through the file. A current engine that is gone or unusable now is
    /// switched to the default choice; a changed one applies to the next
    /// chunk. Replies like `engine_list`, plus `problems`.
    fn engine_reload(&self, engines: &Engines, current: &str) -> Handled {
        let done = engines.reload()?;
        if done.changed.iter().any(|id| id == current) {
            let value = if engines.get(current).is_some() {
                current
            } else {
                engines.default_engine()
            };
            let mut set = Map::new();
            set.insert("key".into(), json!("engine"));
            set.insert("value".into(), json!(value));
            self.set(&set)?;
        }
        let mut f = engines.list(&self.current_engine());
        f.insert("problems".into(), json!(done.problems));
        Ok((f, After::Nothing))
    }

    /// `engine_remove` `{engine, forget_key?}`: a current engine is switched
    /// to the default choice first (saved like any `set`).
    fn engine_remove(&self, engines: &Engines, m: &Map<String, Value>, current: &str) -> Handled {
        let id = opt_str(m, "engine")?
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("missing 'engine'"))?;
        let forget_key = match m.get("forget_key") {
            None | Some(Value::Null) => true,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(bad("'forget_key' must be true or false")),
        };
        if !engines.contains(id) {
            return Err(Failure::new(Code::NotFound, format!("no engine '{id}'")));
        }
        if current == id {
            let mut set = Map::new();
            set.insert("key".into(), json!("engine"));
            set.insert("value".into(), json!(engines.default_engine()));
            self.set(&set)?;
        }
        engines.remove(id, forget_key)?;
        let mut f = Map::new();
        f.insert("removed".into(), json!(id));
        f.insert("engine".into(), json!(self.current_engine()));
        Ok((f, After::Nothing))
    }

    /// `shutdown` (extension `system`, #202): the user's stop (`sonara
    /// stop`, uninstall, an upgrade replacing this runtime). Whatever is
    /// reading or queued ends; requests after it are `E_BUSY`, and the
    /// process exits once the reply went out, restoring other apps first.
    fn shutdown(&self) -> Handled {
        *self.retiring.lock().unwrap_or_else(|p| p.into_inner()) = true;
        let _ = self.reader.control(Control::Stop);
        Ok((Map::new(), After::Exit))
    }

    /// Carry out an accepted takeover or a `shutdown`: called by the
    /// transport after the reply went out.
    pub fn exit_for_takeover(&self) {
        self.lifetime.request_exit(ExitReason::Takeover);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonara_audio::TestOutput;
    use sonara_engine::fake::FakeEngine;
    use sonara_reader::{Config, Registry};
    use std::time::Duration;

    fn server() -> (Server, TestOutput) {
        let registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out.clone()), rx))
                .unwrap();
        let life = Lifetime::new(Duration::from_secs(30), false);
        (Server::new(reader, "secret".into(), life), out)
    }

    fn call(s: &Server, session: &mut Session, req: Value) -> Outcome {
        s.handle(session, &req)
    }

    fn authed(s: &Server) -> Session {
        let mut session = Session::tcp();
        let o = call(s, &mut session, json!({"type": "hello", "token": "secret"}));
        assert_eq!(o.reply["ok"], true);
        session
    }

    fn code(o: &Outcome) -> &str {
        o.reply["error"]["code"].as_str().unwrap_or("")
    }

    #[test]
    fn tcp_needs_hello_with_the_token_first() {
        let (s, _) = server();
        let mut session = Session::tcp();
        let o = call(&s, &mut session, json!({"type": "speak", "text": "Hi."}));
        assert_eq!(code(&o), "E_AUTH");
        assert!(matches!(o.after, After::Close));
        let o = call(&s, &mut session, json!({"type": "hello", "token": "nope"}));
        assert_eq!(code(&o), "E_AUTH");
        assert!(!session.authed);
        let o = call(&s, &mut session, json!("not an object"));
        assert_eq!(code(&o), "E_AUTH");
    }

    #[test]
    fn hello_reports_version_protocol_and_capabilities() {
        let (s, _) = server();
        let mut session = Session::tcp();
        let o = call(
            &s,
            &mut session,
            json!({"type": "hello", "token": "secret", "id": "h1",
                   "client": {"name": "t", "version": "1"},
                   "protocol": {"major": 1, "minor": 0},
                   "extensions": ["channels"], "future_field": 1}),
        );
        let r = &o.reply;
        assert_eq!(r["id"], "h1");
        assert_eq!(r["version"], crate::VERSION);
        assert_eq!(r["protocol"], json!({"major": 1, "minor": 4}));
        assert!(r["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("core")));
        assert_eq!(r["extensions"], json!(["channels"]));
        assert_eq!(r["unavailable"], json!([]));
        assert!(session.authed);
    }

    #[test]
    fn hello_rejects_an_unmet_require_and_another_major() {
        let (s, _) = server();
        let mut session = Session::tcp();
        let o = call(
            &s,
            &mut session,
            json!({"type": "hello", "token": "secret", "require": ["core", "system"]}),
        );
        assert_eq!(code(&o), "E_UNSUPPORTED");
        assert!(!session.authed);
        let o = call(
            &s,
            &mut session,
            json!({"type": "hello", "token": "secret", "protocol": {"major": 2, "minor": 0}}),
        );
        assert_eq!(code(&o), "E_INCOMPATIBLE");
    }

    #[test]
    fn takeover_is_accepted_only_when_idle() {
        let (s, out) = server();
        let mut session = authed(&s);
        call(
            &s,
            &mut session,
            json!({"type": "speak", "text": "One. Two."}),
        );
        let mut other = Session::tcp();
        let hello = json!({"type": "hello", "token": "secret", "takeover": true});
        let o = call(&s, &mut other, hello.clone());
        assert_eq!(code(&o), "E_BUSY");
        call(
            &s,
            &mut session,
            json!({"type": "control", "action": "pause"}),
        );
        let o = call(&s, &mut other, hello.clone());
        assert_eq!(code(&o), "E_BUSY", "a paused item is not idle");
        call(
            &s,
            &mut session,
            json!({"type": "control", "action": "stop"}),
        );
        let _ = out.take_calls();
        let o = call(&s, &mut other, hello);
        assert_eq!(o.reply["ok"], true);
        assert_eq!(o.reply["takeover"], true);
        assert!(matches!(o.after, After::Exit));
    }

    #[test]
    fn after_an_accepted_takeover_speak_and_control_are_busy() {
        let (s, _) = server();
        let mut session = authed(&s);
        let mut other = Session::tcp();
        let o = call(
            &s,
            &mut other,
            json!({"type": "hello", "token": "secret", "takeover": true}),
        );
        assert_eq!(o.reply["ok"], true);
        let o = call(&s, &mut session, json!({"type": "speak", "text": "Late."}));
        assert_eq!(
            code(&o),
            "E_BUSY",
            "a speak after the takeover would be dropped"
        );
        let o = call(
            &s,
            &mut session,
            json!({"type": "control", "action": "play"}),
        );
        assert_eq!(code(&o), "E_BUSY");
        assert!(s.is_idle());
    }

    #[test]
    fn unknown_types_extension_types_and_bad_fields() {
        let (s, _) = server();
        let mut session = authed(&s);
        let o = call(&s, &mut session, json!({"type": "dance", "id": 3}));
        assert_eq!(code(&o), "E_UNKNOWN_TYPE");
        assert_eq!(o.reply["id"], 3);
        let o = call(
            &s,
            &mut session,
            json!({"type": "channel_open", "channel": "a"}),
        );
        assert_eq!(code(&o), "E_UNSUPPORTED");
        let o = call(&s, &mut session, json!({"type": "speak"}));
        assert_eq!(code(&o), "E_BAD_REQUEST");
        let o = call(
            &s,
            &mut session,
            json!({"type": "speak", "text": "a", "mode": "x"}),
        );
        assert_eq!(code(&o), "E_BAD_REQUEST");
        let o = call(
            &s,
            &mut session,
            json!({"type": "control", "action": "fly"}),
        );
        assert_eq!(code(&o), "E_BAD_REQUEST");
        let o = call(
            &s,
            &mut session,
            json!({"type": "control", "action": "next_channel"}),
        );
        assert_eq!(code(&o), "E_UNSUPPORTED");
        let o = call(
            &s,
            &mut session,
            json!({"type": "set", "key": "volume", "value": 101}),
        );
        assert_eq!(code(&o), "E_BAD_REQUEST");
        let o = call(
            &s,
            &mut session,
            json!({"type": "set", "key": "voice", "value": "nobody"}),
        );
        assert_eq!(code(&o), "E_NOT_FOUND");
        let o = call(
            &s,
            &mut session,
            json!({"type": "get", "key": "audio_mode"}),
        );
        assert_eq!(code(&o), "E_UNSUPPORTED");
        let o = call(
            &s,
            &mut session,
            json!({"type": "voices", "engine": "nope"}),
        );
        assert_eq!(code(&o), "E_NOT_FOUND");
        let o = call(&s, &mut session, json!({"no": "type"}));
        assert_eq!(code(&o), "E_BAD_REQUEST");
    }

    #[test]
    fn speak_set_get_and_voices() {
        let (s, _) = server();
        let mut session = authed(&s);
        let o = call(
            &s,
            &mut session,
            json!({"type": "speak", "text": "Hello.", "label": "x", "extra": [1]}),
        );
        assert_eq!(o.reply["item_id"], 1);
        let o = call(
            &s,
            &mut session,
            json!({"type": "set", "key": "volume", "value": 40}),
        );
        assert_eq!(o.reply["value"], 40);
        let o = call(&s, &mut session, json!({"type": "get", "key": "engine"}));
        assert_eq!(o.reply["value"], "fake");
        let o = call(&s, &mut session, json!({"type": "voices"}));
        let voices = o.reply["voices"].as_array().unwrap();
        assert_eq!(voices[0]["engine"], "fake");
        assert_eq!(voices[0]["license_class"], "permissive");
    }

    #[test]
    fn subscribe_is_tcp_only_and_checks_names() {
        let (s, _) = server();
        let mut http = Session::http();
        let o = call(
            &s,
            &mut http,
            json!({"type": "subscribe", "events": ["state"]}),
        );
        assert_eq!(code(&o), "E_BAD_REQUEST");
        let mut session = authed(&s);
        let o = call(
            &s,
            &mut session,
            json!({"type": "subscribe", "events": ["bogus"]}),
        );
        assert_eq!(code(&o), "E_UNSUPPORTED");
        let o = call(
            &s,
            &mut session,
            json!({"type": "subscribe", "events": ["state"]}),
        );
        assert_eq!(o.reply["events"], json!(["state"]));
        let After::Subscribe(mut rx) = o.after else {
            panic!("expected a subscription");
        };
        let first = rx.blocking_recv().unwrap();
        assert_eq!(first.name, "state");
        let v: Value = serde_json::from_str(&first.json).unwrap();
        assert_eq!(v["engine_status"]["engine"], "fake");
    }

    #[test]
    fn keep_alive_in_hello_is_sticky() {
        let (s, _) = server();
        let mut session = Session::tcp();
        call(
            &s,
            &mut session,
            json!({"type": "hello", "token": "secret", "keep_alive": true}),
        );
        assert!(!s.lifetime().may_idle_exit());
    }

    #[test]
    fn an_idle_exit_is_refused_while_reading_and_then_admits_nothing() {
        // #194: the exit decision and a request that starts speech must
        // not interleave (sonarad exited while an item was playing).
        let (s, out) = server();
        let life_idle = Lifetime::new(Duration::ZERO, false);
        let s = Server {
            lifetime: life_idle,
            ..s
        };
        let mut session = Session::http();
        call(
            &s,
            &mut session,
            json!({"type": "speak", "text": "One. Two."}),
        );
        assert!(!s.retire_if_idle(), "reading: no exit");
        assert_eq!(
            code(&call(
                &s,
                &mut session,
                json!({"type": "control", "action": "stop"})
            )),
            ""
        );
        let _ = out.take_calls();
        assert!(s.retire_if_idle(), "idle and expired: exit");
        let o = call(&s, &mut session, json!({"type": "speak", "text": "Late."}));
        assert_eq!(code(&o), "E_BUSY", "nothing is accepted and then dropped");
    }

    #[test]
    fn a_recent_request_keeps_it_from_retiring() {
        let (s, _) = server();
        let s = Server {
            lifetime: Lifetime::new(Duration::from_secs(30), false),
            ..s
        };
        s.lifetime().touch();
        assert!(!s.retire_if_idle());
        s.lifetime().set_keep_alive();
        assert!(!s.retire_if_idle());
    }

    #[test]
    fn token_comparison() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
    }
}
