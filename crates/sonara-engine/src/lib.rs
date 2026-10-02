//! Sonara L1 engines: the `Engine` trait every speech engine implements, the
//! types it speaks in, a `Registry` that enforces the licence rule (R6), and
//! the engines themselves (`onecore`; `kokoro` behind its feature; `fake`
//! for tests).
//!
//! An engine turns one chunk of text into PCM. It knows nothing about items,
//! queues or playback: the reader state machine (`sonara_core::reader`) asks
//! for a chunk with `Effect::Synthesize` and the host calls `synthesize`.
mod error;
mod registry;
mod types;
pub mod wav;

pub mod onecore;

#[cfg(feature = "kokoro")]
pub mod kokoro;

#[cfg(feature = "test-util")]
pub mod fake;

pub use error::{Error, Result, MISSING_VOICE_DATA_FIX};
pub use registry::Registry;
pub use types::{EngineId, EngineStatus, LicenseClass, PcmChunk, Readiness, Voice};

/// The PCM chunks of one synthesis, in playback order.
pub type PcmStream = Box<dyn Iterator<Item = Result<PcmChunk>> + Send>;

/// A speech engine. Implementations are shared between the thread that
/// synthesizes and the one that cancels, hence `Send + Sync`.
pub trait Engine: Send + Sync {
    fn id(&self) -> EngineId;
    /// Fixed per engine; the `Registry` refuses classes its host does not allow.
    fn license_class(&self) -> LicenseClass;
    /// Every voice this engine offers, installed or not.
    fn voices(&self) -> Vec<Voice>;
    /// Prepare for a fast first synthesis and surface setup problems early
    /// (for OneCore: missing voice data).
    fn warm(&self) -> Result<()>;
    /// Synthesize `text` with `voice` (a `Voice::id` or name; empty for the
    /// engine default) at `rate` (Sonara words per minute, 200 = normal).
    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream>;
    /// Abandon every synthesis in flight; each ends with `Error::Cancelled`.
    fn cancel(&self);
    /// Readiness (spec 4.1 `engine_status`); cheap, polled by the reader.
    fn status(&self) -> EngineStatus {
        EngineStatus::ready()
    }
}
