# Text-rule golden cases

Input and expected output for Sonara's text rules (`cleaner.py`, `assembler.py`), one JSON file per category. `tests/test_text_rules_golden.py` runs every case against the Python code. A port of these rules (for example TypeScript in an embedding host) is correct when it passes the same files.

Each file is `{"description": str, "cases": [case, ...]}`. A case has a `name`, an `fn` and:

- `fn: "clean_markdown"` or `"normalize_for_speech"`: `input` (string) maps to `output` (string).
- `fn: "assemble"`: `deltas` (list of strings) are fed to one `ProseAssembler` as one message block, delta `i` with index `i`, and the last delta with `final: true`. `output` is every chunk emitted across all feeds, in order; `null` marks a paragraph break.

The outputs record current behaviour, quirks included (a table row keeps its pipes, emoji pass through). Change a case only together with a deliberate change to the rule.
