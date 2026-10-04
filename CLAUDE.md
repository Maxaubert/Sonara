# Sonara

Eyes-free text-to-speech for Claude Code, Windows only. Since 0.11 (#202) the Claude Code plugin runs the Rust runtime (`crates/`) with no Python: `hooks/hooks.json` -> `bin/sonara-hook-launch` (Git Bash) -> `sonara-hook.exe` -> `sonarad.exe`; `commands/*.md` -> `bin/sonara` -> `sonara.exe` (`crates/sonara-cli`). The Python package `src/sonara` (>= 3.9) stays until a follow-up removes it; the plugin does not use it.

## Remotes
- `origin` = the fork `Maxaubert/Sonara` (PRs go here). `upstream` = `nimkimi/sonari`: never push there.
- `gh` default repo is `Maxaubert/Sonara`. Issue numbers in commits and docs refer to the fork.

## Build, test, release
- Typecheck/lint: `ruff check src tests conformance clients/python packaging` (no Python typechecker; TS: `npm run typecheck` in `clients/ts`)
- Rust (`crates/`, needs `~/.cargo/bin` on PATH): `cargo fmt --all -- --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace; cargo deny check licenses bans`. Live OneCore (opt-in): `cargo test -p sonara-engine --test onecore_live -- --ignored`. Live L4 checks on this PC's audio, media and hotkeys (opt-in): `cargo test -p sonara-system --test win_live -- --ignored`. End to end: `cargo run -p sonara-reader --example say -- --engine onecore|fake "Hello."`
- External engines (#224): conformance and SDK tests run `sonarad --keys fake` (keys in `<home>ake-keys.json`, never Credential Manager). Live (opt-in, keys from env vars, each skips when unset): `cargo test -p sonara-engine --test external_live -- --ignored --nocapture` (`OPENAI_API_KEY`, `SONARA_LIVE_KOKORO_FASTAPI_URL`, `SONARA_LIVE_LOCALAI_URL`+`_MODEL`, `SONARA_LIVE_SPEACHES_URL`+`_MODEL`); Credential Manager round trip: `cargo test -p sonara-engine --test credman_live -- --ignored`
- Kokoro engine (M4): `python packaging/runtime_dlls.py stage target/<debug|release>` puts `onnxruntime.dll` (pinned SHA-256) and the VC++ DLLs it needs next to `sonarad.exe` (without them sonarad falls back to OneCore). Live check: `SONARA_KOKORO_MODELS=<folder with the two model files> cargo test -p sonara-engine --release --test kokoro_live -- --ignored --nocapture`. G2P golden changes: `SONARA_BLESS=1 cargo test -p sonara-engine --test kokoro_g2p`, then review the diff. Vendored G2P data: `crates/misaki/tools/pack_data.py`. The CRT is linked statically (`.cargo/config.toml`).
- Protocol v1 conformance (black box, CI rust job): `cargo build -p sonarad -p sonara-hook -p sonara-cli; python -m pytest conformance -q` (`--engine fake --system fake`, temp `SONARA_HOME`; contract `docs/protocol-v1.md`). `conformance/plugin/` drives `bin/` in Git Bash and the PowerShell bootstrap against a local release server, `sonara.exe`, and a release zip installed into a temp `LOCALAPPDATA`
- SDKs (CI clients job, Node 18 + Python 3.9): `cargo build -p sonarad --release`, then `cd clients/ts && npm ci && npm run build && npm test` (`test:unit` needs no sonarad), `cd clients/player && npm ci && npm run typecheck && npm run build && npm test` (demo: `examples/player-demo`, see `clients/player/README.md`), `python -m pytest clients/python/tests -q`, `cd packaging/npm-runtime && npm run build && npm test`, `node packaging/smoke/run-node.mjs`. Bundling guide: `docs/bundling.md`
- Earcons (#211): the bundled WAVs are `crates/sonara-agent/sounds/<kind>.wav`, rendered by `python packaging/sounds/build_earcons.py` (numpy + scipy; `--check` compares with `SHA256SUMS`). Never commit the user's reference recordings.
- Notices (R6): after any Rust dependency change run `python packaging/notices/gen_notices.py` (CI `--check`); models/data notices are hand-kept in `packaging/notices/models-and-data.md`
- Unit: `python -m pytest -q` (system Python with `.[dev,windows]`; conftest adds `src/` to `sys.path`). Live OneCore checks: `-m live_windows` (opt-in)
- E2E (headless): `python -m pytest tests/e2e -q` (needs `pip install -e ".[e2e]"` and `playwright install chromium`)   Run when: `crates/sonarad/assets/settings.html`, `crates/sonarad/src/settings_page.rs` (needs `cargo build -p sonarad`), legacy `src/sonara/settings.html`, `src/sonara/webui.py`
- Build / package: the plugin is the repo (marketplace `.claude-plugin/`); runtime zip `cargo build -p sonarad -p sonara-hook -p sonara-cli --release; python packaging/runtime_dlls.py stage target/release; python packaging/release_zip.py` (zip + `SHA256SUMS`, both attached by release.yml)   Artifact: `sonara-runtime-win-x64-<version>.zip`; npm/PyPI publishing is manual (`docs/bundling.md`). Order: merge -> release.yml publishes `v<version>` -> users install or update (the launcher downloads the release named in `bin/runtime-version`)
- Known failures to tolerate: none
- Version source: `pyproject.toml` + `src/sonara/__init__.py` + `.claude-plugin/plugin.json` + `.claude-plugin/marketplace.json` + `bin/runtime-version` + `Cargo.toml` `[workspace.package]` + `clients/ts/package.json` + `clients/player/package.json` + `clients/ts/src/version.ts` + `packaging/npm-runtime/package.json` + `clients/python/pyproject.toml` + `clients/python/src/sonara_client/version.py` (keep equal, `test_manifests.py`)   Release: release.yml on push to main (CI: ci.yml, Python 3.9 + 3.12)
- Install locally after merge: `/plugin update sonara@sonara` once the release exists, restart Claude Code   Confirm version: `/sonara:doctor` (its `version` row)
- Deploy: plugin marketplace
- Signing: unsigned

## Runtime deploy drift (the biggest gotcha)
- The plugin runs the runtime in `%LOCALAPPDATA%\Sonara\runtime\<bin/runtime-version>\`, NOT the repo or `target/`. Before diagnosing behaviour: `sonara.exe version` there vs the branch. The home (settings, logs, `runtime.json`) is `%LOCALAPPDATA%\Sonara` (`SONARA_HOME` overrides).
- Safe redeploy of a branch build (same version folder; never during someone's session):
  1. `cargo build -p sonarad -p sonara-hook -p sonara-cli --release; python packaging/runtime_dlls.py stage target/release`
  2. `"$LOCALAPPDATA/Sonara/runtime/<ver>/sonara.exe" stop` (writes `stopped`, restores ducked apps, waits for the exit).
  3. Copy `target/release/{sonarad,sonara-hook,sonara}.exe` and the staged DLLs into that folder.
  4. `"$LOCALAPPDATA/Sonara/runtime/<ver>/sonara.exe" start` (clears `stopped`).
- Hooks run through Git Bash: `bin/sonara-hook-launch` reads `bin/runtime-version`, execs `sonara-hook.exe` (adds `--standalone` to `SONARA_RUNTIME_ARGS`), else starts `bin/sonara-bootstrap.ps1` once in the background (lock `runtime\.bootstrap.lock`, retry marker `.bootstrap.failed`, 5 min). Test overrides: `SONARA_RELEASE_BASE_URL`, `SONARA_BOOTSTRAP_START=0`, `SONARA_NO_BROWSER`.
- Logs: `%LOCALAPPDATA%\Sonara\logs\` (crate `sonara-log`, #219: 1 MB segments, 10 MB for the whole folder, oldest first out): `sonarad.log` (activity per #217; `in` messages, `agent` decisions, `read text` spoken text, `drop` reasons), `hook.log` (each hook's raw payload and what it sent), `bootstrap.log`. Text only while setting `debug_log` is on (default). Settings: `config.json` there (only user-set keys; product defaults in `sonarad::config::SCHEMA`); change keys live through the settings page (`sonara.exe settings`) or protocol `set`.
- The Python product (until removed): daemon copy in `~/.sonara/app`, redeploy with `from sonara.install.app_copy import copy_app`; it is no longer installed by the plugin.

## Architecture
Map, threads, lock contract, module owners and how to add a setting/message/hotkey: `docs/architecture.md`. Rules that prevent mistakes:
- Hooks never import the daemon: `hooks_entry.py` -> `client.py` -> TCP (token in `~/.sonara/daemon.lock`); `client.py` starts the daemon via `lifecycle.py`.
- Wire protocol: `docs/protocol.md` is the contract for embedding hosts. Changes stay additive; update the doc with any message change. Text-rule golden cases for ports: `tests/fixtures/text_rules/`.
- `handle_message` is table dispatch: each `MsgType` has one owner module, registered via `core.add_handlers` (owners are listed in `docs/architecture.md`; SET_AUDIO_MODE/SET_DUCK_LEVEL/SET_VOLUME belong to `audio`, not `settings`).
- New per-session state MUST be registered in `core.SessionRegistry`: session teardown is `forget_session(sid)` and nothing is hand-listed.
- Feature modules keep daemon state by reference: never rebind it on the daemon, mutate in place. Persist via `daemon._persist()`. Never block under the daemon lock.
- Tests patch names on the module that owns them (`install/` modules call each other via module attributes; the platform via `sonara.platform.get_platform`).
- Settings: one table in `config_schema.py` feeds DEFAULTS, the daemon, webui and CLI. `config.json` stores only user-set keys; bundled earcons resolve at runtime, never stored.
- PRIVACY.md lists every file in `%LOCALAPPDATA%\Sonara`: update it when adding one.
- `cli.py`: argparse + thin command functions only.

## Product rules
- One message, always the last: Sonara reads the latest turn; Up restarts it. Nothing may silently drop it.
- Default hotkeys stay Ctrl+Alt+Up/Down/M/P (#160): Windows owns Win+Alt+arrows/M/P. Ctrl+Alt is AltGr on European layouts, so a clash is a doctor/settings warning whose fix is a rebind with Win, not a reset.
- Never leave other apps ducked or paused.

## Conventions
- `daemon/`, `install/`, `webui.py`, `cli.py` and `summarizer.py` stay OS-free: no `sys.platform`/`os.name` branch, win32 import or `platform.windows` import (`test_no_os_branch_in_core.py` enforces it). The summarizer's child-process flags, PATHEXT lookup and tree kill come from `platform.child_processes()`.
- Python 3.9 syntax (`test_py39_compat.py`), `from __future__ import annotations`.
- Every `~/.sonara` path goes through `paths.py` (conftest isolates it per test). conftest also points `~/.claude/settings.json` and the launcher dir at tmp and refuses mutating `schtasks`: a test that misses a platform patch reaches the real supervisor.
- Bug fixes are test-first, with a regression test named after the behaviour.
- No em-dashes anywhere (code, comments, docs, commit messages).
- Text rules exist in Python and Rust until M9; change both together with the golden fixtures. Likewise until `src/sonara` is removed (the follow-up to M11, #202): the summarizer prompts (`crates/sonara-agent/prompts/`, `test_agent_prompts.py`) and the hook mapping (`crates/sonara-hook/tests/golden/`, `test_hook_golden.py`).
- Current work: external TTS engines (#224-#227), spec and plan `docs/plans/2026-10-04-external-engines-spec.md`. Before that, the Rust reader runtime, spec `docs/plans/2026-10-02-sonara-runtime-spec.md`, plan `docs/plans/2026-10-02-sonara-runtime-plan.md` (Phase 0 is done: `docs/plans/phase0-plan.md`). Research: `docs/plans/2026-10-02-distribution-research.md`, `docs/plans/embedding-research.md`. Historical specs, plans and audits: `docs/history/`.
