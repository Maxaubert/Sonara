use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Language {
    EnglishUS,
}

impl Language {
    pub fn is_english(&self) -> bool {
        matches!(self, Language::EnglishUS)
    }

    /// The vendored crate ships only the US English lexicon (Sonara).
    pub fn is_british(&self) -> bool {
        false
    }
}
