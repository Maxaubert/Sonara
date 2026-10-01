# Sonara

Eyes-free text-to-speech for Claude Code, Windows only. Python >= 3.9 (`src/sonara`), shipped as a Claude Code plugin.

## Remotes
- `origin` = the fork `Maxaubert/Sonara` (PRs go here). `upstream` = `nimkimi/sonari`: never push there.
- `gh` default repo is `Maxaubert/Sonara`. Issue numbers in commits and docs refer to the fork.

## Build, test, release
- Typecheck/lint: `ruff check src tests` (no typechecker)
- Unit: `python -m pytest -q` (system Python with `.[dev,windows]`; conftest adds `src/` to `sys.path`). Live OneCore checks: `-m live_windows` (opt-in)
- E2E (headless): `python -m pytest tests/e2e -q` (needs `playwright install chromium`)   Run when: `src/sonara/settings.html`, `src/sonara/webui.py`
- Build / package: none (plugin, no build step)   Artifact: n/a
- Known failures to tolerate: none
- Version source: `pyproject.toml` + `.claude-plugin/plugin.json` + `.claude-plugin/marketplace.json` (keep equal, `test_manifests.py`)   Release: release.yml on push to main (CI: ci.yml, Python 3.9 + 3.12)
- Install locally after merge: safe redeploy below   Confirm version: `sonara doctor`
- Deploy: plugin marketplace
- Signing: unsigned

## Runtime deploy drift (the biggest gotcha)
- The daemon runs the deployed copy in `~/.sonara/app/sonara`, NOT the repo. Before diagnosing behaviour: `diff -rq ~/.sonara/app/sonara src/sonara`. Python caches modules, so a redeploy needs a daemon restart.
- Safe redeploy (from the repo or a worktree):
  1. `PYTHONPATH=src python -m sonara.cli shutdown`, then wait until no `pythonw.exe` remains.
  2. `PYTHONPATH=src python -c "from sonara.cli import _copy_app; _copy_app(r'<repo>')"`
  3. If only `sonara.old` / `sonara.new` remain (#127), rename `sonara.new` to `sonara`.
  4. `PYTHONPATH=~/.sonara/app python -m sonara.cli start` (starting with `PYTHONPATH=src` runs the REPO copy).
- Hooks run through Git Bash: `bin/sonara-hook-run` picks the interpreter (`~/.sonara/python.path`, then a non-Store PATH python, then `py -3`) and runs `bin/sonara-hook` on it. `bin/sonara-hook.cmd` is not used by `hooks/hooks.json`. settings.json installs get exec-form hooks generated from `hooks/hooks.json`.
- Logs: `~/.sonara/speechd.log`, `~/.sonara/faulthandler.log`. Config: `~/.sonara/config.json`; change keys live with POST `http://127.0.0.1:27431/api/set` and the token in `~/.sonara/webui.token`.

## Architecture map
- `hooks/hooks.json` -> `bin/sonara-hook-run` -> `bin/sonara-hook` -> `hooks_entry.py` (pure event translation) -> `client.py` -> TCP (token in `~/.sonara/daemon.lock`) -> `daemon/`. Hooks never import the daemon: `client.py` starts it via `lifecycle.py`.
- `daemon/` package (split in progress, #141): `__init__.py` is still the bulk (`SpeechDaemon`: message handling, speak loop, summary pipeline; `main`). Split out: `decision_text`, `tokens`, `setup_health`, `summary/reorder` (digest order), `core` (debug lock check, `SONARA_DEBUG_LOCKS=1`), `cues` (CONTROL-channel cues, cue voice), `audio` (duck/pause, volume), `hotkeys` (listener, worker, debounce), `server` (socket, token, connection cap). Feature modules get shared state passed in (`daemon._cues`, `_audio`, `_hotkeys`, `_server`). Tests patch names on the module that now owns them. `router.py` + `channel.py`: per-session channels. `speaker.py`: playback and cancel epochs.
- `assembler.py`, `cleaner.py`: text to spoken items. `summarizer.py`: `claude -p` / `codex exec` digests.
- Persisted state under `~/.sonara`, every path via `paths.py`: history, sessions, session prefs, digests.
- Settings: one table in `config_schema.py` (default, validator, page path, live-apply hook) feeds config DEFAULTS, the daemon, webui and CLI. `config.json` stores only user-set keys (pre-#136 full dumps: values equal to a current or past default count as unset). Bundled earcons resolve at runtime, never stored.
- `platform/`: OS seam (`base.py` + `windows/`: tts, hotkeys, earcons, ducking, pausing, supervisor = install, autostart, hooks).
- `webui.py` + `settings.html`: token-protected settings page. `cli.py`: CLI verbs, install, doctor. `install_record.py`: install.json. `platform/windows/process.py`: daemon process hardening. `kokoro*.py`: neural voices.

## Product rules
- One message, always the last: Sonara reads the latest turn; Up restarts it. Nothing may silently drop it.
- Never leave other apps ducked or paused.

## Conventions
- Core stays OS-free: no win32 imports outside `platform/windows` (`test_no_os_branch_in_core.py`).
- Python 3.9 syntax (`test_py39_compat.py`), `from __future__ import annotations`.
- Every `~/.sonara` path goes through `paths.py` (conftest isolates it per test).
- Bug fixes are test-first, with a regression test named after the behaviour.
- No em-dashes in user-facing text.
- Current work plan: `docs/plans/phase0-plan.md`. Historical specs, plans and audits: `docs/history/`.
