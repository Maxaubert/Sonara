# Sonara docs

Start here. Using Sonara? Open the user guide. Working on the code? `CLAUDE.md` holds the rules and gates, `CONTRIBUTING.md` the PR rules, and this index says where everything else lives.

## For users

| Doc | Purpose |
|---|---|
| [user/README.md](user/README.md) | User guide: install, hotkeys, settings, voices and engines, summaries, sessions, troubleshooting, FAQ |

## For contributors and embedders

| Doc | Purpose | Status |
|---|---|---|
| [architecture.md](architecture.md) | Crate map (layer, owns, deps, key files, tests), process chain, threads, locks and the lock order, how-to recipes (setting, message, hotkey, engine kind, earcon), log pointers | current |
| [protocol-v1.md](protocol-v1.md) | Wire contract for embedding hosts (TCP and HTTP, extensions), contents at the top, version table (1.0 to 1.11). Changes stay additive | current |
| [protocol-v1-engines.md](protocol-v1-engines.md) | External engines (capability `engines`): profiles, keys, one section per kind, the `engine_*` messages, the voice rule | current |
| [bundling.md](bundling.md) | Embedding the runtime in npm and PyPI hosts | current |
| [testing.md](testing.md) | Live tests and their env vars, Kokoro and G2P, conformance, SDK steps, the embedder e2e suite, earcons, notices, version files, safe redeploy | current |
| [plans/](plans/) | The one open plan: [question follow](plans/2026-10-16-question-follow.md) (issue #323). Finished specs and plans were removed from the tree | open plan |
| [history/architecture-python.md](history/architecture-python.md) | Architecture of the retired Python daemon | history |

Everything else that used to live in `docs/history/` (Python-era plans, specs, audits, mockups and
checklists) and in `docs/plans/` (the finished runtime and engine specs, research notes) was removed
from the tree. The full old tree is at the git tag `history-final`:
`git show history-final:docs/history/README.md`.

## Other places

- Crates: each `crates/<name>/src/lib.rs` opens with a `//!` header that says what the crate owns.
- Clients: `clients/ts`, `clients/player` and `clients/python` each have a README.
- Repo root: `README.md` (users), `CONTRIBUTING.md` (gates and PR rules), `PRIVACY.md` (every file
  Sonara writes), `LICENSING.md` and `THIRD_PARTY_NOTICES.md`.
- Research reports are kept outside the repo, in the maintainer's notes; the repo keeps only the
  short facts it needs.
