# Contributing to Sonara

Sonara is a Windows-only tool. Verification has two layers: the test suite checks the logic,
and a human checks the Windows runtime on real hardware. Start with
[docs/architecture.md](docs/architecture.md) for how the code fits together.

## Branch model

- **`main` is the trunk and is always releasable.** There are no long-lived integration
  branches.
- **One issue, one branch, one PR.** Branch off `main` as `type/<issue>-slug`
  (`fix/128-up-always-restart`, `feat/...`, `refactor/...`, `docs/...`, `ci/...`).
  Commits are `type(scope): subject (#issue)`. A version bump or tooling change needed by a
  change rides inside that change's PR.
- **Bump the version in every PR**: patch for fixes, minor for features, in `pyproject.toml`,
  `src/sonara/__init__.py`, `.claude-plugin/plugin.json` and `.claude-plugin/marketplace.json`
  (`tests/test_manifests.py` keeps them equal). A push to `main` runs `release.yml`, which
  publishes `v<version>` and refuses a version that already exists.
- **Squash-merge into `main`**, then delete the branch locally and on the remote.

## 1. The test suite (runs anywhere)

The suite uses fakes for speech, audio and hotkeys, so it needs no speech engine and runs
headless. Before opening a PR:

```powershell
python -m venv .venv
.venv\Scripts\pip install -e ".[dev,windows]"
.venv\Scripts\ruff check src tests
.venv\Scripts\python -m pytest -q
```

Both must be green. CI (`.github/workflows/ci.yml`) runs on Windows: the unit suite on Python
3.9 and 3.12, plus ruff on 3.12 (lock checks, `SONARA_DEBUG_LOCKS=1`, are a local diagnostic
since #250). Tests must not depend on what is
installed on your PC (Kokoro, Windows voices): patch the platform and `kokoro.is_installed`.

- **Settings page changes** (`settings.html`, `webui.py`) also need the browser tests:
  `pip install -e ".[e2e]"`, `playwright install chromium`, then `python -m pytest tests/e2e -q`.
  CI skips them, so they are the local gate.
- **Real speech checks** are marked `live_windows` and run only on request:
  `python -m pytest -m live_windows`.
- **Bug fixes are test-first**: a regression test named after the behaviour, failing before the
  fix.

## 2. Runtime acceptance (a human, on Windows)

The suite proves nothing about real speech, the daemon's crash and restart paths, the global
hotkeys, earcon mixing, ducking or autostart. For a change that touches runtime behaviour,
deploy the branch to `~/.sonara/app` (see the safe redeploy steps in `CLAUDE.md`), use it in a
real Claude Code session, and say in the PR what you tested.

## Code rules

- The core stays OS-free. Windows code lives behind the platform seam in
  `src/sonara/platform/` (`tests/test_no_os_branch_in_core.py` enforces it).
- Python 3.9 syntax (`tests/test_py39_compat.py`).
- Every `~/.sonara` path goes through `paths.py`.
- Protocol changes are additive and update `docs/protocol.md`.
- No em-dashes in user-facing text.

## A PR merges when

1. It is one concern, branched off `main`.
2. Ruff and the test suite are green, locally and in CI.
3. A maintainer has approved it.
4. If it touches runtime behaviour, it has been tested on real hardware.

## Behaviour changes

Sonara is an eyes-free tool, so changes to core controls (hotkeys, what gets spoken, default
bindings) are user-facing decisions. Call them out in the PR description with a
**Behaviour change:** line, and raise anything that removes or remaps a default before you
build it.
