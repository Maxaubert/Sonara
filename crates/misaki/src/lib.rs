//! A vendored, trimmed copy of misaki-rs 0.6.0 (MIT, Michele Yin,
//! https://github.com/MicheleYin/misaki-rs), the Rust port of hexgrad's
//! misaki G2P (Apache-2.0, https://github.com/hexgrad/misaki).
//!
//! Changes from upstream, for Sonara's Kokoro engine (runtime M4, #200):
//! - US English only, with the lexicons and tagger weights stored
//!   xz-compressed (`data/`, `tools/pack_data.py`) instead of plain JSON;
//! - no espeak-ng fallback and no `espeak-rs` dependency (spec R2/R6); the
//!   host supplies its own `Fallback`;
//! - the unused `language-tokenizer` (WTFPL) and `fancy-regex` dependencies
//!   are gone;
//! - `Lexicon::insert_gold` lets a host add its own words before misaki's;
//!   `G2P::with_lexicon` shares one lexicon with the host's fallback.
//!
//! The upstream code is kept close to its original form, so clippy's style
//! lints are allowed here.
#![allow(clippy::all)]

pub mod data;
pub mod fallback;
pub mod g2p;
pub mod language;
pub mod languages;
pub mod lexicon;
pub mod tagger;
pub mod token;

pub use fallback::Fallback;
pub use g2p::G2P;
pub use language::Language;
pub use lexicon::Lexicon;
pub use token::MToken;
