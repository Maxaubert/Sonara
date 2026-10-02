//! Engine errors. Messages are written for the person reading a log or the
//! `say` example: each names the cause and, where there is one, the repair.
use crate::types::{EngineId, LicenseClass};

pub type Result<T> = std::result::Result<T, Error>;

/// How to restore OneCore voice data (D7, `docs/plans/phase0-plan.md`).
pub const MISSING_VOICE_DATA_FIX: &str = "Fix: Settings > Time & language > Speech > Manage \
voices, remove and add English (United States) again, or in an elevated prompt run: DISM \
/Online /Add-Capability /CapabilityName:Language.TextToSpeech~~~en-US~0.0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The host's licence policy does not allow this engine (R6).
    #[error("engine '{engine}' refused: licence class {class:?} is not allowed by this host")]
    LicenseRefused {
        engine: EngineId,
        class: LicenseClass,
    },
    #[error("engine '{0}' is already registered")]
    DuplicateEngine(EngineId),
    #[error("no engine '{0}' is registered")]
    UnknownEngine(String),
    /// OneCore lists no voice. On a PC whose voice data is gone this is what
    /// a new program sees: OneCore keeps a per-program copy of the voice
    /// list, so only programs that ran while the voices worked still list
    /// them (and then fail with `MissingVoiceData`).
    #[error(
        "no usable Windows voices: none are installed, or their voice data is missing from \
         this PC. {fix}",
        fix = MISSING_VOICE_DATA_FIX
    )]
    NoVoices,
    /// Voices are listed but their data files are gone, so synthesis fails
    /// with "file not found" (D7: seen on a real PC with David, Zira, Mark).
    #[error(
        "{voices} listed but cannot speak: their voice data is missing from this PC \
         (synthesis fails with 'file not found'). {fix}",
        fix = MISSING_VOICE_DATA_FIX
    )]
    MissingVoiceData { voices: String },
    #[error("unknown voice '{0}'")]
    UnknownVoice(String),
    #[error("synthesis cancelled")]
    Cancelled,
    #[error("invalid WAV data: {0}")]
    Wav(String),
    /// Any other engine failure, with the platform's message.
    #[error("{0}")]
    Engine(String),
}
