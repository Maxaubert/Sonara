use thiserror::Error;

#[derive(Error, Debug)]
pub enum FallbackError {
    #[error("no phonemes matched for '{word}'")]
    NoPhonemes { word: String },
    #[error("mutex poisoned: {0}")]
    MutexPoisoned(String),
}

/// Trait for OOV (out-of-vocabulary) word fallback mechanisms
pub trait Fallback: Send + Sync {
    /// Convert unknown word to phonemes
    /// Returns phonemes
    fn phonemize(&self, word: &str) -> Result<String, FallbackError>;
}
