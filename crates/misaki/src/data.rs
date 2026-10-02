//! The embedded data: the US English lexicons and the POS tagger, stored
//! xz-compressed (`data/*.xz`, built by `tools/pack_data.py` from the
//! misaki-rs 0.6.0 files) and decoded when a lexicon or tagger is built.
use crate::lexicon::PhonemeEntry;
use std::collections::HashMap;
use std::io::Read;

pub const US_GOLD_XZ: &[u8] = include_bytes!("../data/us_gold.json.xz");
pub const US_SILVER_XZ: &[u8] = include_bytes!("../data/us_silver.json.xz");
const TAGGER_WEIGHTS_XZ: &[u8] = include_bytes!("../data/tagger-weights.json.xz");
const TAGGER_TAGS_XZ: &[u8] = include_bytes!("../data/tagger-tags.json.xz");
pub const TAGGER_CLASSES: &str = include_str!("../data/tagger-classes.txt");

/// Decode one embedded xz file. The data is part of the binary, so a
/// failure is a build defect, not a runtime condition.
pub fn unxz(packed: &[u8]) -> String {
    let mut out = String::new();
    lzma_rust2::XzReader::new(packed, false)
        .read_to_string(&mut out)
        .expect("embedded misaki data is valid xz and UTF-8");
    out
}

fn table(packed: &[u8]) -> HashMap<String, PhonemeEntry> {
    serde_json::from_str(&unxz(packed)).expect("embedded misaki lexicon is valid JSON")
}

pub fn load_us_gold() -> HashMap<String, PhonemeEntry> {
    table(US_GOLD_XZ)
}

pub fn load_us_silver() -> HashMap<String, PhonemeEntry> {
    table(US_SILVER_XZ)
}

pub fn tagger_weights() -> String {
    unxz(TAGGER_WEIGHTS_XZ)
}

pub fn tagger_tags() -> String {
    unxz(TAGGER_TAGS_XZ)
}
