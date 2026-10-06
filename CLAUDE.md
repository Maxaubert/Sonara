# Sonara

Eyes-free text-to-speech for Claude Code, Windows only, run by the Rust runtime in `crates/`. Hooks: `hooks/hooks.json` -> `bin/sonara-hook-launch` (Git Bash) -> `sonara-hook.exe` -> `sonarad.exe`. Commands: `commands/*.md` -> `bin/sonara` -> `sonara.exe` (`crates/sonara-cli`). Docs index: `docs/README.md`.

## Remotes
- `origin` = the fork `Maxaubert/Sonara` (PRs go here). `upstream` = `nimkimi/sonari`: never push there.
- `gh` default repo is `Maxaubert/Sonara`. Issue numbers in commits and docs refer to the fork.

## Build, test, release
- Gate script (#256): `python packaging/gate.py` runs the gates the changed paths need, as CI does (`--dry-run` plan, `--quick` changed crates only, `--all` everything); run it before every commit.
- Rust (needs `~/.cargo/bin` on PATH): `cargo fmt --all -- --check; cargo clippy --workspace --all-targets -- -D warnings; cargo nextest run --workspace; cargo test --workspace --doc; cargo deny check licenses bans` (without cargo-nextest: `cargo test --workspace`). One crate: `cargo nextest run -p <crate>`.
- Conformance (protocol v1, black box): `cargo build -p sonarad -p sonara-hook -p sonara-cli; python -m pytest conformance -q` (the newest of `target/release` and `target/debug` unless `SONARAD`/`SONARA_HOOK` are set).
- CI (`ci.yml`) jobs, the required checks: `check`, `rust`, `conformance`, `clients`, `deny`; keep the names (`packaging/gate.py` `REQUIRED_CHECKS`).
- Typecheck/lint: `ruff check tests conformance clients/python packaging`; TS: `npm run typecheck` in `clients/ts`.
- Repo checks (`tests/repo`, Python 3.12, `pip install --group dev`; `pyproject.toml` is dev config only): `python -m pytest tests/repo -q` (plain `python -m pytest -q` also runs `tests/e2e`).
- E2E (headless): `python -m pytest tests/e2e -q` (`pip install --group e2e`, `playwright install chromium`). Run when: `crates/sonarad/assets/settings/` (the page in parts, joined in `settings_page.rs`), `crates/sonarad/src/settings_page.rs` or `config.rs` change (needs `cargo build -p sonarad`).
- `clients/`, `packaging/npm-runtime` or a version file changed: run the SDK gates. Rust dependency changed: regenerate notices. Live tests, Kokoro, G2P bless, earcons, SDK steps: `docs/testing.md`.
- Embedder e2e (#274): `python -m pytest tests/embed -q -rs` after the release build of all three exes and `runtime_dlls.py stage` (gate `embed`; runs when the runtime, SDKs, packaging or `docs/protocol-v1.md`/`bundling.md` change). Hosts on the packed npm packages, the wheel in a fresh venv and the release zip; `sonarad --output wav:<dir>` lets them check real audio. Opt-in: `SONARA_EMBED_RELEASE=1` for the latest GitHub release.
- Engine contract tests (#275, no network, in `cargo test`): `crates/sonara-engine/tests/contract_*.rs` check each adapter against fragments of the provider's official spec in `tests/contracts/<provider>/` (each with `SOURCES.md`); refresh with `packaging/contracts/extract_fragment.py` (`docs/testing.md`).
- Skill `sonara-gates`: the gate script, every gate command and when the e2e and SDK gates apply.
- Skill `sonara-live-tests`: the opt-in live checks and their env vars.
- Build / package: `cargo build -p sonarad -p sonara-hook -p sonara-cli --release; python packaging/runtime_dlls.py stage target/release; python packaging/release_zip.py`   Artifact: `sonara-runtime-win-x64-<version>.zip` + `SHA256SUMS`
- Known failures to tolerate: none
- Version: `Cargo.toml` `[workspace.package]`; `python packaging/bump_version.py <version>` sets every version file and lockfile entry (`--check` prints it, as release.yml reads it; `tests/repo/test_manifests.py`)   Release: release.yml once ci.yml passed on a push to main (`workflow_run`, #250); users get the release named in `bin/runtime-version`
- Install locally after merge: `/plugin update sonara@sonara` once the release exists, restart Claude Code   Confirm version: `/sonara:doctor` (its `version` row)
- Deploy: plugin marketplace   Signing: unsigned

## Runtime deploy drift (the biggest gotcha)
- The plugin runs `%LOCALAPPDATA%\Sonara\runtime\<bin/runtime-version>\`, NOT the repo or `target/`. Before diagnosing behaviour compare `sonara.exe version` there with the branch. Redeploy a branch build only with `sonara.exe stop` first and `start` after the copy, never during someone's session.
- Skill `sonara-redeploy`: the safe stop, copy and start of a branch build, in the same or a new version folder, and the rollback.
- Home (`SONARA_HOME` overrides): `%LOCALAPPDATA%\Sonara`: `config.json` (only user-set keys), `runtime.json`, `logs\` (`sonarad.log`, `hook.log`, `bootstrap.log`).

## Architecture (map: `docs/architecture.md`)
- Layers: L1 `sonara-core`, `-engine`, `-audio`, `-reader`, `misaki`, `sonara-log`; L2 `sonara-channels`; L3 `sonara-agent`; L4 `sonara-system`; L5 `sonara-hook`, `sonara-cli` on the leaf protocol client `sonara-client`; `sonarad` hosts them. No crate depends upward (one table of allowed edges: `crates/sonara-core/tests/layering.rs`).
- Add a setting: `sonarad::config::SCHEMA` (`crates/sonarad/src/config.rs`), then the page (`assets/settings/`; ranges and defaults come from `SCHEMA`), then e2e.
- Add a message: dispatch in `crates/sonarad/src/protocol/mod.rs` plus the `*_ext.rs` `TYPES` (`EXTENSION_TYPES` chains them); document it in `docs/protocol-v1.md` (the contract, changes additive only), add conformance tests, update both SDKs (`clients/ts`, `clients/python`).
- Lock order across crates: agent rules > seen > channels state > schedule > player > reader (`docs/architecture.md`). Never take a lock to the left while holding one to the right; L2 `on_drop` runs off-lock, `on_announce` and L3 `on_trace` run under a lock and must not call back in.
- PRIVACY.md lists every file in `%LOCALAPPDATA%\Sonara`: update it when adding one.

## Product rules
- One message, always the last: Sonara reads the latest turn; Up restarts it. Nothing may silently drop it.
- Default hotkeys stay Ctrl+Alt+Up/Down/M/P (#160): Windows owns Win+Alt+arrows/M/P. Ctrl+Alt is AltGr on European layouts, so a clash is a doctor/settings warning whose fix is a rebind with Win, not a reset.
- Never leave other apps ducked or paused.
- External engines: never put a model id or voice name in code, defaults, hints or docs (#235, user decision 2026-10-05): they come from the provider's live lists or are typed; examples use `<model>`/`<voice>`.

## Conventions
- No em-dashes anywhere (code, comments, docs, commit messages).
- Bug fixes are test-first, with a regression test named after the behaviour.
- Golden fixtures: text rules `tests/fixtures/text_rules/` (`crates/sonara-core/tests/golden.rs`), hook mapping `crates/sonara-hook/tests/golden/` with the captured payloads in `tests/fixtures/`. Change a case only with a deliberate rule change.

## Current work
- Repo cleanup after the 2026-10-05 audit (the Python package `src/sonara` was removed in #248).
