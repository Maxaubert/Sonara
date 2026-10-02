//! The value types engines speak in.
use std::fmt;

/// Names an engine (`onecore`, `kokoro`, `fake`). Engines are compiled in,
/// so the id is a static string; the protocol's `engine` setting compares
/// against `as_str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EngineId(pub &'static str);

impl EngineId {
    pub fn as_str(&self) -> &'static str {
        self.0
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
}

impl EngineStatus {
    pub fn ready() -> Self {
        EngineStatus {
            readiness: Readiness::Ready,
            progress: None,
            fallback: None,
            message: None,
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
