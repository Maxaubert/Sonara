# Echo Phase 1 — Review Follow-ups (MUST be resolved before Phase 1 is "done")

This file is the durable backlog of concerns spotted during subagent-driven
execution. **The final whole-implementation review MUST read this file and
verify or fix every open item**, then check it off. Do not consider Phase 1
complete while any item is unchecked.

Each item: what, where, why it matters, and the concrete action + how to verify.

---

## Open

- [ ] **`load_config` may return a shallow copy of `DEFAULTS` (shared nested dicts).**
  - **Where:** `src/echo/config.py` (`load_config` / `_deep_merge`). Section 2,
    task "load_config returns DEFAULTS copy when CONFIG_PATH missing". During
    execution a deep-copy fix (`d720499`) was added then **reverted** (`08d2ee6`)
    to satisfy a literal spec reading; that task ended `specPass:false`.
  - **Why it matters:** if `load_config()` returns a config whose nested `earcons`
    dict is the *same object* as the module-level `DEFAULTS["earcons"]`, then any
    code that mutates `cfg["earcons"][k]` (or the daemon updating config in place)
    silently corrupts `DEFAULTS` for the rest of the process — a classic
    shared-mutable-default bug.
  - **Action:** confirm `load_config()` returns a fully independent copy (deep-copy
    nested dict values). Add a regression test: load a config, mutate a nested
    value (e.g. `cfg["earcons"]["choice"] = "X"`), reload/inspect `DEFAULTS` and
    assert it is unchanged. Reconcile with `save_config`/daemon mutation paths.

- [ ] **Re-audit every task that ended `specPass:false`.** The spec-review loop
  did not converge on these; confirm each is actually correct (not a real gap):
  - Section 1: "Create venv, editable install, and verify pytest" (added then the
    follow-up reverted an `*.egg-info/` .gitignore entry — verify the working tree
    stays clean after `pip install -e .` and that egg-info never gets committed).
  - Section 2: "load_config returns DEFAULTS copy when CONFIG_PATH missing" (see
    the deep-copy item above).
  - Section 3: 5 of 7 tasks ended `specPass:false` — **already audited & cleared** by the
    controller (direct probe of `ProseAssembler`: sentence assembly, dedup, final-flush, and
    code-fence summary all correct; the e2e prose case matches). Flags were pedantic reactions
    to legitimate corrections (plan's "1-line" fence count was wrong → "2-line"; a commit msg).
    No action needed for §3.

## Resolved

(none yet)
