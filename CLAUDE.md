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
- Hooks run through Git Bash: `bin/sonara-hook` under console `python.exe`. `bin/sonara-hook.cmd` is not used by `hooks/hooks.json`.
- Logs: `~/.sonara/speechd.log`, `~/.sonara/faulthandler.log`. Config: `~/.sonara/config.json`; change keys live with POST `http://127.0.0.1:27431/api/set` and the token in `~/.sonara/webui.token`.

## Architecture map
- `hooks/hooks.json` -> `bin/sonara-hook` -> `hooks_entry.py` (pure event translation) -> `client.py` -> TCP (token in `~/.sonara/daemon.lock`) -> `daemon.py`.
- `daemon.py`: message handling, speak loop, summary pipeline, hotkeys, audio. `router.py` + `channel.py`: per-session channels. `speaker.py`: playback and cancel epochs.
- `assembler.py`, `cleaner.py`: text to spoken items. `summarizer.py`: `claude -p` / `codex exec` digests.
- Persisted state under `~/.sonara`, every path via `paths.py`: history, sessions, session prefs, digests.
- `platform/`: OS seam (`base.py` + `windows/`: tts, hotkeys, earcons, ducking, pausing, supervisor = install, autostart, hooks).
- `webui.py` + `settings.html`: token-protected settings page. `cli.py`: CLI verbs, install, doctor. `kokoro*.py`: neural voices.

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
