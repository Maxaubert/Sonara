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

#[cfg(any(feature = "kokoro", feature = "external"))]
pub mod http;

#[cfg(feature = "external")]
pub mod external;

pub mod onecore;

#[cfg(feature = "kokoro")]
pub mod kokoro;

#[cfg(feature = "test-util")]
pub mod fake;

pub use error::{Error, Result, MISSING_VOICE_DATA_FIX};
pub use registry::Registry;
pub use types::{EngineId, EngineStatus, LicenseClass, PcmChunk, Readiness, Reason, Voice};

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
    /// The caller is about to call `synthesize` from this thread: a
    /// `cancel` from now on ends that synthesis too, even one that has not
    /// started yet (the reader calls it under its queue lock, which a
    /// cancel also takes). Default: nothing (a synthesis that starts after
    /// a cancel runs; fine for a local engine).
    fn begin(&self) {}
    /// Readiness (spec 4.1 `engine_status`); cheap, polled by the reader.
    fn status(&self) -> EngineStatus {
        EngineStatus::ready()
    }
    /// How many chunks the reader should synthesize ahead of the playing
    /// one (1..=4). A cloud engine asks for more to hide its round trip.
    fn lookahead(&self) -> usize {
        1
    }
    /// Join the reader's sentences into chunks of up to this many
    /// characters (#235; the first chunk of an item stays one sentence).
    /// An engine whose provider counts requests (Gemini's free tier) asks
    /// for fewer, longer chunks. Default 0: one sentence per chunk.
    fn chunk_chars(&self) -> usize {
        0
    }
    /// With `chunk_chars`: true (the default) keeps the first chunk of an
    /// item one sentence, so reading starts at once; false joins every
    /// chunk up to `chunk_chars`, so a reply under it is one request and
    /// reading starts once all of it is made (review of #235).
    fn quick_start(&self) -> bool {
        true
    }
    /// True when the `PcmStream` of `synthesize` yields audio while the
    /// rest is still being made (#235, Gemini's streamed answer): the
    /// reader then starts playing a chunk at its first piece instead of
    /// waiting for all of it. Default false: the reader collects the
    /// stream first.
    fn streams(&self) -> bool {
        false
    }
    /// True when `synthesize` accepts voice ids that `voices()` does not
    /// list (cloud voice ids, cloned voices, file names of a local server).
    fn accepts_unlisted_voices(&self) -> bool {
        false
    }
    /// Fetch the voice list from its source (for an external engine: the
    /// network, bounded by a timeout; it may block). `voices()` stays cheap
    /// and returns the last list.
    fn refresh_voices(&self) -> Result<Vec<Voice>> {
        Ok(self.voices())
    }
}
