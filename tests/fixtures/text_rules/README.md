# Text-rule golden cases

Input and expected output for Sonara's text rules (`crates/sonara-core/src/text.rs`, `assembler.rs`), one JSON file per category. `crates/sonara-core/tests/golden.rs` runs every case against the Rust code. A port of these rules (for example TypeScript in an embedding host) is correct when it passes the same files.

Each file is `{"description": str, "cases": [case, ...]}`. A case has a `name`, an `fn` and:

- `fn: "clean_markdown"` or `"normalize_for_speech"`: `input` (string) maps to `output` (string).
- `fn: "assemble"`: `deltas` (list of strings) are fed to one `ProseAssembler` as one message block, delta `i` with index `i`, and the last delta with `final: true`. `output` is every chunk emitted across all feeds, in order; `null` marks a paragraph break.
- `fn: "assemble"` with `feeds` instead of `deltas`: a list of `[delta, index, final]`, fed in order to one `ProseAssembler`, for a gap, a late delta or a resent index (`streaming.json`).

The outputs record current behaviour, quirks included (`clean_markdown` keeps a table row's pipes, emoji pass through; the assembler reads tables row by row, `tables.json`). Change a case only together with a deliberate change to the rule.
