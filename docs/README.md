# Sonara docs

Start here. `CLAUDE.md` holds the rules and gates; this index says where everything else lives.

| Doc | Purpose | Status |
|---|---|---|
| [architecture.md](architecture.md) | Crate map (layer, owns, deps, key files, tests), process chain, threads, locks and the lock order, how-to recipes (setting, message, hotkey, engine kind, earcon), log pointers | current |
| [protocol-v1.md](protocol-v1.md) | Wire contract for embedding hosts (TCP and HTTP, extensions), contents at the top. Changes stay additive | current |
| [protocol-v1-engines.md](protocol-v1-engines.md) | External engines (capability `engines`): profiles, keys, one section per kind, the `engine_*` messages, the voice rule | current |
| [bundling.md](bundling.md) | Embedding the runtime in npm and PyPI hosts | current |
| [testing.md](testing.md) | Live tests and their env vars, Kokoro and G2P, conformance, SDK steps, the embedder e2e suite, earcons, notices, version files, safe redeploy | current |
| [plans/](plans/) | Specs and plans: external engines (`2026-10-04-external-engines-spec.md`), Rust runtime spec and plan (`2026-10-02-sonara-runtime-*.md`) | current |
| [history/](history/README.md) | Finished plans, specs, audits and checklists. Ignored by rg and Grep (`/.ignore`) | history |
| [history/architecture-python.md](history/architecture-python.md) | Architecture of the retired Python daemon | history |

Other places:

- Crates: each `crates/<name>/src/lib.rs` opens with a `//!` header that says what the crate owns.
- Clients: `clients/ts`, `clients/player` and `clients/python` each have a README.
- Repo root: `README.md` (users), `CONTRIBUTING.md` (gates and PR rules), `PRIVACY.md` (every file
  Sonara writes), `LICENSING.md` and `THIRD_PARTY_NOTICES.md`.
- Research outside the repo: `research/sonara/` in the maintainer's notes (repo audit
  2026-10-05). `plans/2026-10-02-distribution-research.md` and `plans/embedding-research.md` are
  the in-repo research notes.
