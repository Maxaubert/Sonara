# misaki (vendored)

A trimmed copy of [misaki-rs](https://github.com/MicheleYin/misaki-rs) 0.6.0 (MIT, Copyright (c) 2026 Michele Yin; `LICENSE`), the Rust port of hexgrad's [misaki](https://github.com/hexgrad/misaki) G2P, used by Sonara's Kokoro engine (`crates/sonara-engine`, feature `kokoro`).

Changes from upstream (also listed in `src/lib.rs`):

- US English only. The lexicons (`data/us_gold.json.xz`, `data/us_silver.json.xz`, from misaki by hexgrad, Apache-2.0) and the part-of-speech tagger data are stored xz-compressed and decoded with lzma-rust2 when the G2P is built. `tools/pack_data.py` rebuilds them from the upstream crate and checks its SHA-256 values; `tests/data.rs` checks that they decode to exactly the upstream bytes.
- No espeak-ng fallback and no `espeak-rs` dependency (Sonara spec R2/R6). The host passes its own `Fallback`.
- The unused `language-tokenizer` (WTFPL) and `fancy-regex` dependencies are gone, and so is a debug print.
- `Lexicon::insert_gold` (a host's own words first) and `G2P::with_lexicon` (one lexicon shared with the host's fallback).

Data provenance (espeak-assisted lexicon corrections, the tagger's training data) is recorded in `packaging/notices/models-and-data.md`.
