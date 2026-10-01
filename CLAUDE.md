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
  2. `PYTHONPATH=src python -c "from sonara.install.app_copy import copy_app; copy_app(r'<repo>')"`
  3. If only `sonara.old` / `sonara.new` remain (#127), rename `sonara.new` to `sonara`.
  4. `PYTHONPATH=~/.sonara/app python -m sonara.cli start` (starting with `PYTHONPATH=src` runs the REPO copy).
- Hooks run through Git Bash: `bin/sonara-hook-run` picks the interpreter (`~/.sonara/python.path`, then a non-Store PATH python, then `py -3`) and runs `bin/sonara-hook` on it. `bin/sonara-hook.cmd` is not used by `hooks/hooks.json`. settings.json installs get exec-form hooks generated from `hooks/hooks.json`.
- Logs: `~/.sonara/speechd.log`, `~/.sonara/faulthandler.log`. Config: `~/.sonara/config.json`; change keys live with POST `http://127.0.0.1:27431/api/set` and the token in `~/.sonara/webui.token`.

## Architecture map
- `hooks/hooks.json` -> `bin/sonara-hook-run` -> `bin/sonara-hook` -> `hooks_entry.py` (pure event translation) -> `client.py` -> TCP (token in `~/.sonara/daemon.lock`) -> `daemon/`. Hooks never import the daemon: `client.py` starts it via `lifecycle.py`.
- `daemon/` package (#141): `__init__.py` is the facade (`SpeechDaemon`: wiring, core state such as lock, wake, paused, mute level, ids and heard-markers, `_enqueue`/`_replay`/`note_spoken`, `handle_message`, `run`/`stop`). `handle_message` is a table dispatch: each feature registers its `MsgType`s via `core.add_handlers` (one owner per type, unknown types return None). Handlers: `ingest` (prose, decisions, earcons, FLUSH, session lifecycle; owns assemblers, `await_choice`), `controls` (pause, mute, skip, stop, session switch, flush to end, Up, repeat), `settings` (SET_*, STATUS, `set_config_value`, `set_summary_prompt`), `audio`, `hotkeys`. `core`: `SessionRegistry` (every per-session dict/set or hook is registered there; `_teardown_session` = `forget_session(sid)`, so new per-session state MUST be registered), `SharedState` (current item, Up re-read text), `add_handlers`, debug lock check (`SONARA_DEBUG_LOCKS=1`). Also: `startup` (`main`, single-instance guard), `decision_text`, `tokens`, `setup_health`, `summary/reorder`, `summary/pipeline`, `playback` (speak loop), `cues`, `server`. Handler modules (`ingest`, `controls`, `settings`) get the daemon and read its state at call time; the other feature modules get shared state passed in and keep it by reference: never rebind it on the daemon. Persist via `daemon._persist()`. Tests patch names on the module that now owns them. `router.py` + `channel.py`: per-session channels. `speaker.py`: playback and cancel epochs.
- `assembler.py`, `cleaner.py`: text to spoken items. `summarizer.py`: `claude -p` / `codex exec` digests.
- Persisted state under `~/.sonara`, every path via `paths.py`: history, sessions, session prefs, digests.
- Settings: one table in `config_schema.py` (default, validator, page path, live-apply hook) feeds config DEFAULTS, the daemon, webui and CLI. `config.json` stores only user-set keys (pre-#136 full dumps: values equal to a current or past default count as unset). Bundled earcons resolve at runtime, never stored.
- `platform/`: OS seam. `get_platform()` gives the backends (`base.py` + `windows/`: tts, hotkeys, earcons, ducking, pausing, supervisor = autostart task, launcher, stray-daemon sweep); `daemon_process()` gives `windows/process.py` (faulthandler, priority, VC preload, single-instance guard from `windows/singleton.py`) before any backend loads. `transport.py` is OS-free TCP + lockfile.
- `install/` (#142): `installer` (install, uninstall), `app_copy` (plugin root, runtime copy), `deps` (daemon interpreter, PyWinRT), `service` (stop/start around file changes), `voices`, `cleanup`, `doctor`, `claude_hooks` (settings.json hooks generated from `hooks/hooks.json`; the Windows supervisor calls it). Modules call each other via module attributes: tests patch the owning module, and the platform via `sonara.platform.get_platform`.
- `webui.py` + `settings.html`: token-protected settings page. `cli.py`: argparse + thin command functions only. `install_record.py`: install.json. `kokoro*.py`: neural voices.

## Product rules
- One message, always the last: Sonara reads the latest turn; Up restarts it. Nothing may silently drop it.
- Default hotkeys stay Ctrl+Alt+Up/Down/M/P (#160): Windows owns Win+Alt+arrows/M/P. Ctrl+Alt is AltGr on European layouts, so a clash is a doctor/settings warning whose fix is a rebind with Win, not a reset.
- Never leave other apps ducked or paused.

## Conventions
- `daemon/`, `install/`, `webui.py` and `cli.py` stay OS-free: no `sys.platform`/`os.name` branch, win32 import or `platform.windows` import (`test_no_os_branch_in_core.py` enforces it). `summarizer.py` still branches on `os.name` for its process flags; that is a known gap, not a pattern to copy.
- Python 3.9 syntax (`test_py39_compat.py`), `from __future__ import annotations`.
- Every `~/.sonara` path goes through `paths.py` (conftest isolates it per test). conftest also points `~/.claude/settings.json` and the launcher dir at tmp and refuses mutating `schtasks`: a test that misses a platform patch reaches the real supervisor.
- Bug fixes are test-first, with a regression test named after the behaviour.
- No em-dashes in user-facing text.
- Current work plan: `docs/plans/phase0-plan.md`. Historical specs, plans and audits: `docs/history/`.
