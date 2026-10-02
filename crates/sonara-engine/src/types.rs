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
