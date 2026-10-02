# Sonara

Eyes-free text-to-speech for Claude Code, Windows only. Python >= 3.9 (`src/sonara`), shipped as a Claude Code plugin.

## Remotes
- `origin` = the fork `Maxaubert/Sonara` (PRs go here). `upstream` = `nimkimi/sonari`: never push there.
- `gh` default repo is `Maxaubert/Sonara`. Issue numbers in commits and docs refer to the fork.

## Build, test, release
- Typecheck/lint: `ruff check src tests conformance` (no typechecker)
- Rust (`crates/`, needs `~/.cargo/bin` on PATH): `cargo fmt --all -- --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace; cargo deny check licenses bans`. Live OneCore (opt-in): `cargo test -p sonara-engine --test onecore_live -- --ignored`. End to end: `cargo run -p sonara-reader --example say -- --engine onecore|fake "Hello."`
- Protocol v1 conformance (black box, CI rust job): `cargo build -p sonarad -p sonara-hook; python -m pytest conformance -q` (`--engine fake`, temp `SONARA_HOME`; contract `docs/protocol-v1.md`)
- Unit: `python -m pytest -q` (system Python with `.[dev,windows]`; conftest adds `src/` to `sys.path`). Live OneCore checks: `-m live_windows` (opt-in)
- E2E (headless): `python -m pytest tests/e2e -q` (needs `pip install -e ".[e2e]"` and `playwright install chromium`)   Run when: `src/sonara/settings.html`, `src/sonara/webui.py`
- Build / package: none (plugin, no build step)   Artifact: n/a
- Known failures to tolerate: none
- Version source: `pyproject.toml` + `src/sonara/__init__.py` + `.claude-plugin/plugin.json` + `.claude-plugin/marketplace.json` + `Cargo.toml` `[workspace.package]` (keep equal, `test_manifests.py`)   Release: release.yml on push to main (CI: ci.yml, Python 3.9 + 3.12)
- Install locally after merge: safe redeploy below   Confirm version: `sonara doctor` (its `version` row)
- Deploy: plugin marketplace
- Signing: unsigned

## Runtime deploy drift (the biggest gotcha)
- The daemon runs the deployed copy in `~/.sonara/app/sonara`, NOT the repo. Before diagnosing behaviour: `diff -rq ~/.sonara/app/sonara src/sonara`. Python caches modules, so a redeploy needs a daemon restart.
- Safe redeploy (from the repo or a worktree):
  1. `PYTHONPATH=src python -m sonara.cli shutdown`, then wait until no `pythonw.exe` remains.
  2. `PYTHONPATH=src python -c "from sonara.install.app_copy import copy_app; copy_app(r'<repo>')"`
  3. If only `sonara.old` / `sonara.new` remain (#127), rename `sonara.new` to `sonara`.
  4. `PYTHONPATH=~/.sonara/app python -m sonara.cli start` (starting with `PYTHONPATH=src` runs the REPO copy).
- Hooks run through Git Bash: `bin/sonara-hook-run` picks the interpreter (`~/.sonara/python.path`, then a non-Store PATH python, then `py -3`) and runs `bin/sonara-hook` on it. `bin/sonara-hook.cmd` is not used by `hooks/hooks.json`. settings.json installs get exec-form hooks generated from `hooks/hooks.json`.
- Logs: `~/.sonara/speechd.log`, `~/.sonara/faulthandler.log`. Config: `~/.sonara/config.json`; change keys live with POST `http://127.0.0.1:27431/api/set` and the token in `~/.sonara/webui.token`.

## Architecture
Map, threads, lock contract, module owners and how to add a setting/message/hotkey: `docs/architecture.md`. Rules that prevent mistakes:
- Hooks never import the daemon: `hooks_entry.py` -> `client.py` -> TCP (token in `~/.sonara/daemon.lock`); `client.py` starts the daemon via `lifecycle.py`.
- Wire protocol: `docs/protocol.md` is the contract for embedding hosts. Changes stay additive; update the doc with any message change. Text-rule golden cases for ports: `tests/fixtures/text_rules/`.
- `handle_message` is table dispatch: each `MsgType` has one owner module, registered via `core.add_handlers` (owners are listed in `docs/architecture.md`; SET_AUDIO_MODE/SET_DUCK_LEVEL/SET_VOLUME belong to `audio`, not `settings`).
- New per-session state MUST be registered in `core.SessionRegistry`: session teardown is `forget_session(sid)` and nothing is hand-listed.
- Feature modules keep daemon state by reference: never rebind it on the daemon, mutate in place. Persist via `daemon._persist()`. Never block under the daemon lock.
- Tests patch names on the module that owns them (`install/` modules call each other via module attributes; the platform via `sonara.platform.get_platform`).
- Settings: one table in `config_schema.py` feeds DEFAULTS, the daemon, webui and CLI. `config.json` stores only user-set keys; bundled earcons resolve at runtime, never stored.
- PRIVACY.md lists every `~/.sonara` file: update it when adding one.
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
- Text rules exist in Python and Rust until M9; change both together with the golden fixtures. Likewise until M11: the summarizer prompts (`crates/sonara-agent/prompts/`, `test_agent_prompts.py`) and the hook mapping (`crates/sonara-hook/tests/golden/`, `test_hook_golden.py`).
- Current work: the Rust reader runtime, spec `docs/plans/2026-10-02-sonara-runtime-spec.md`, plan `docs/plans/2026-10-02-sonara-runtime-plan.md` (Phase 0 is done: `docs/plans/phase0-plan.md`). Research: `docs/plans/2026-10-02-distribution-research.md`, `docs/plans/embedding-research.md`. Historical specs, plans and audits: `docs/history/`.
