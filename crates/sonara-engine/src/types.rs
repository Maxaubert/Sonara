//! The value types engines speak in.
use std::collections::HashSet;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// Names an engine (`onecore`, `kokoro`, `fake`, or the id of an external
/// engine profile). Built-in engines use a static string; profile ids are
/// interned (`EngineId::intern`), so the id stays `Copy`. The protocol's
/// `engine` setting compares against `as_str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EngineId(pub &'static str);

impl EngineId {
    pub fn as_str(&self) -> &'static str {
        self.0
    }

    /// The id for a run-time name (an external engine profile): the same
    /// `&'static str` for the same text, leaked once per distinct id per
    /// process. Callers validate ids first (at most 16 profiles of at most
    /// 32 bytes), so the set stays small.
    pub fn intern(id: &str) -> EngineId {
        static IDS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
        let mut ids = IDS
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(s) = ids.get(id) {
            return EngineId(s);
        }
        let s: &'static str = Box::leak(id.to_string().into_boxed_str());
        ids.insert(s);
        EngineId(s)
    }
}

impl fmt::Display for EngineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// What a bundler may ship (R6). Copyleft has no variant on purpose: an
/// engine that would need one cannot be written against this trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LicenseClass {
    /// Code and data under a permissive licence (MIT, Apache-2.0, BSD...).
    Permissive,
    /// Part of the operating system, nothing shipped (Windows OneCore).
    Os,
    /// An engine outside Sonara the user added at run time (a cloud API, a
    /// local server or program): nothing shipped, the text leaves Sonara.
    /// Hosts that bundle Sonara may refuse it.
    External,
}

/// Why an external engine did not speak a chunk itself (spec 8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reason {
    NoKey,
    Auth,
    Quota,
    RateLimited,
    Network,
    Timeout,
    Server,
    BadVoice,
    BadConfig,
    Format,
}

impl Reason {
    pub const ALL: [Reason; 10] = [
        Reason::NoKey,
        Reason::Auth,
        Reason::Quota,
        Reason::RateLimited,
        Reason::Network,
        Reason::Timeout,
        Reason::Server,
        Reason::BadVoice,
        Reason::BadConfig,
        Reason::Format,
    ];

    /// The wire name (`engine_status.reason`, `error.reason`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Reason::NoKey => "no_key",
            Reason::Auth => "auth",
            Reason::Quota => "quota",
            Reason::RateLimited => "rate_limited",
            Reason::Network => "network",
            Reason::Timeout => "timeout",
            Reason::Server => "server",
            Reason::BadVoice => "bad_voice",
            Reason::BadConfig => "bad_config",
            Reason::Format => "format",
        }
    }

    /// A failure that may pass on its own (the breaker counts it); the
    /// others block the engine until the user changes something.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Reason::RateLimited | Reason::Network | Reason::Timeout | Reason::Server
        )
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How text goes to an engine (#235, "Send to the engine"): a whole
/// message in one request, or each sentence as it comes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SendMode {
    /// Each sentence is its own request (the reader's chunks), read as soon
    /// as it is made: the built-in engines, programs and local servers.
    #[default]
    Sentence,
    /// What the agent releases for speech at once (a reply at its end, a
    /// summary, the prose before a question) is joined into one request,
    /// split only past the provider's input limit (`Engine::input_limit`),
    /// and played as its audio streams in: the cloud engines, which bill
    /// or limit per request.
    Message,
}

impl SendMode {
    pub const ALL: [SendMode; 2] = [SendMode::Message, SendMode::Sentence];

    /// The wire name (`send_mode` of a profile).
    pub fn as_str(&self) -> &'static str {
        match self {
            SendMode::Sentence => "sentence",
            SendMode::Message => "message",
        }
    }

    pub fn parse(s: &str) -> Option<SendMode> {
        SendMode::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

impl fmt::Display for SendMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The most text one request may carry (spec 13.1 "Input limits").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputLimit {
    Chars(usize),
    /// UTF-8 bytes (Google).
    Bytes(usize),
}

impl InputLimit {
    /// The size of `s` in this limit's unit.
    pub fn size(&self, s: &str) -> usize {
        match self {
            InputLimit::Chars(_) => s.chars().count(),
            InputLimit::Bytes(_) => s.len(),
        }
    }

    /// The limit (at least 1).
    pub fn max(&self) -> usize {
        match self {
            InputLimit::Chars(n) | InputLimit::Bytes(n) => (*n).max(1),
        }
    }

    /// Whether `s` fits.
    pub fn fits(&self, s: &str) -> bool {
        self.size(s) <= self.max()
    }
}

/// One voice of one engine (protocol `voices`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    /// Stable id, accepted by `Engine::synthesize`.
    pub id: String,
    /// Display name.
    pub name: String,
    /// BCP 47 tag, e.g. `en-US`.
    pub language: String,
    pub engine: EngineId,
    pub license_class: LicenseClass,
    /// False when the voice is listed but cannot speak yet (a model still to
    /// download, OneCore voice data missing).
    pub installed: bool,
}

/// Interleaved 16-bit PCM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmChunk {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl PcmChunk {
    /// Length in milliseconds (0 for an empty or malformed chunk).
    pub fn duration_ms(&self) -> u64 {
        let per_second = self.sample_rate as u64 * self.channels as u64;
        if per_second == 0 {
            return 0;
        }
        self.samples.len() as u64 * 1000 / per_second
    }
}

/// How far an engine is from speaking with its own voice (spec 4.1
/// `state.engine_status`). Engines that are always ready (OneCore, fake)
/// keep the trait's default, `EngineStatus::ready()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Readiness {
    Ready,
    /// Loading its model (seconds).
    Loading,
    /// Downloading its model; `EngineStatus::progress` says how far.
    Downloading,
    /// The last download or load failed; it tries again later
    /// (`EngineStatus::message` says why).
    Waiting,
    /// It cannot run in this install (for Kokoro: no ONNX Runtime).
    Unavailable,
}

impl Readiness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Readiness::Ready => "ready",
            Readiness::Loading => "loading",
            Readiness::Downloading => "downloading",
            Readiness::Waiting => "waiting",
            Readiness::Unavailable => "unavailable",
        }
    }
}

/// An engine's readiness, with what speaks meanwhile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStatus {
    pub readiness: Readiness,
    /// Bytes done and expected while a model downloads.
    pub progress: Option<(u64, u64)>,
    /// The engine that speaks while this one is not ready (Kokoro falls
    /// back to OneCore).
    pub fallback: Option<EngineId>,
    /// Why it is not ready, when something failed.
    pub message: Option<String>,
    /// For an external engine: the class of the failure (spec 8.1).
    pub reason: Option<Reason>,
}

impl EngineStatus {
    pub fn ready() -> Self {
        EngineStatus {
            readiness: Readiness::Ready,
            progress: None,
            fallback: None,
            message: None,
            reason: None,
        }
    }
}

impl fmt::Display for EngineStatus {
    /// For log lines: "downloading its model (40%), speaking with onecore
    /// meanwhile".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.readiness {
            Readiness::Ready => f.write_str("ready")?,
            Readiness::Loading => f.write_str("loading its model")?,
            Readiness::Downloading => {
                f.write_str("downloading its model")?;
                if let Some((done, total)) = self.progress.filter(|(_, t)| *t > 0) {
                    write!(f, " ({}%)", done.saturating_mul(100) / total)?;
                }
            }
            Readiness::Waiting => f.write_str("not ready, retrying later")?,
            Readiness::Unavailable => f.write_str("unavailable")?,
        }
        if let Some(m) = &self.message {
            write!(f, ": {m}")?;
        }
        match (self.readiness, self.fallback) {
            (Readiness::Ready, _) | (_, None) => Ok(()),
            (_, Some(e)) => write!(f, "; speaking with {e} meanwhile"),
        }
    }
}
