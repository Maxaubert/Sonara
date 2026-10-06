# Sonara docs

Start here. `CLAUDE.md` holds the rules and gates; this index says where everything else lives.

| Doc | Purpose | Status |
|---|---|---|
| [architecture.md](architecture.md) | Process chain, threads and locks, how the code fits together. The Rust crate map and recipes are being rewritten (#253); the Python sections are legacy | partly legacy |
| [protocol-v1.md](protocol-v1.md) | Wire contract for embedding hosts (TCP and HTTP, extensions, external engines). Changes stay additive | current |
| [bundling.md](bundling.md) | Embedding the runtime in npm and PyPI hosts | current |
| [testing.md](testing.md) | Live tests and their env vars, Kokoro and G2P, conformance, SDK steps, earcons, notices, version files, safe redeploy | current |
| [plans/](plans/) | Specs and plans: external engines (`2026-10-04-external-engines-spec.md`), Rust runtime spec and plan (`2026-10-02-sonara-runtime-*.md`) | current |
| [history/](history/README.md) | Finished plans, specs, audits and checklists. Ignored by rg and Grep (`/.ignore`) | history |
| [protocol.md](protocol.md) | Wire protocol of the retired Python daemon | legacy, deleted with `src/sonara` (#248) |

Other places:

- Crates: each `crates/<name>/src/lib.rs` opens with a `//!` header that says what the crate owns.
- Clients: `clients/ts`, `clients/player` and `clients/python` each have a README.
- Repo root: `README.md` (users), `CONTRIBUTING.md` (gates and PR rules), `PRIVACY.md` (every file
  Sonara writes), `LICENSING.md` and `THIRD_PARTY_NOTICES.md`.
- Research outside the repo: `research/sonara/` in the maintainer's notes (repo audit
  2026-10-05). `plans/2026-10-02-distribution-research.md` and `plans/embedding-research.md` are
  the in-repo research notes.
