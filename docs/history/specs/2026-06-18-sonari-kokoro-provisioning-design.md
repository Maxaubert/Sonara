# Sonari Kokoro neural-voice provisioning - design

**Date:** 2026-06-18
**Status:** approved (provisioning approach = uv-managed Python)
**Relates to:** #41 (macOS Kokoro playback, merged PR #52), #42 (Windows Kokoro), #53 (download earcon), #54 (actionable-by-ear errors)

## Problem

Kokoro neural voices were wired to run **in-process** in the speech daemon (#41/#42). But the daemon's interpreter is chosen by `supervisor.resolve_python()`, which deliberately prefers the always-present, stable **system `/usr/bin/python3` (3.9)**. Kokoro cannot run there:

- **Empirically proven this session:** `kokoro-onnx` (every published version, 0.1.6 → 0.5.0) requires `onnxruntime>=1.20.1` **and** `numpy>=2.0.2`. Both dropped Python 3.9 (onnxruntime's last 3.9 wheel is 1.19.2; numpy 2.x needs ≥3.10). `pip install` on 3.9 → `ResolutionImpossible`. No pin escapes it.
- The working combo (e2e-verified on Python 3.12/3.13 this session): `kokoro-onnx 0.5.0`, `onnxruntime 1.27`, `numpy 2.4.6`, pulling `espeakng-loader 0.2.4` + `phonemizer-fork 3.3.2` transitively. The bundled espeak-ng works - **no system espeak-ng required**.

So enabling neural means provisioning a **stable Python ≥3.10 environment** with the extra, and pointing the daemon at it. Today that's a manual `pip` into some modern Python - which "won't fly on a new machine." Goal: **one opt-in command, zero host prerequisite**, that a new machine's claude-everywhere bootstrap can run unattended.

## Approach (decided)

Keep Kokoro **in-process** (consistent with the merged #41/#42 design). Provision the ≥3.10 environment with **uv** (chosen over "discover an existing modern Python" and "download python-build-standalone ourselves" because only a zero-prerequisite provisioner achieves the new-machine goal, and uv solves the OS×arch Python-acquisition matrix we'd otherwise own). Base (non-neural) users are untouched: no command, daemon stays on system 3.9, zero dependencies.

## Design

### Command surface
- **`sonari voices install [kokoro]`** - provision + enable neural. Idempotent.
- **`sonari voices uninstall`** - remove the venv, revert the daemon to system Python, restart.
- **`/sonari:install --with-kokoro`** (or a `/sonari:voices` slash command) - eyes-free wrapper.
- **`sonari doctor`** gains a neural-readiness row (venv present, kokoro importable in the venv, model downloaded, daemon running on the venv interpreter).

### Provisioning (uv-managed)
1. **Ensure uv.** Use `uv` on PATH if present; else bootstrap it (`/usr/bin/python3 -m pip install uv`, which ships a 3.9-compatible binary wrapper; fall back to the official installer). A missing/failed uv bootstrap is a clean, actionable error - never a half-wired state.
2. **Create the venv:** `uv venv ~/.sonari/venv --python 3.12` (uv downloads the managed CPython if absent - this is the zero-prerequisite step).
3. **Install pinned deps** into the venv from a checked-in `requirements-kokoro.txt` (pinned to the e2e-verified combo) so every machine gets the same validated set. Do **not** pip-install the `sonari` package itself into the venv.
4. The daemon then runs **`~/.sonari/venv/bin/python -m sonari.daemon` with `PYTHONPATH=APP_DIR`**: `sonari` imports from `APP_DIR` (still the single source of code, so `sonari install` updates keep working), `kokoro_onnx`/`numpy`/`onnxruntime` import from the venv's own site-packages.

### Daemon interpreter selection (the wiring change)
- Introduce a neural-aware interpreter choice: **if neural is enabled** (the venv python exists and probes ≥3.10) **→ venv python; else `resolve_python()` (3.9)**.
- `install()` uses this selector, so re-running `sonari install` after enabling neural **keeps** the venv interpreter instead of resetting to 3.9.
- `voices install` writes both LaunchAgents with the venv interpreter (reusing the existing supervisor install path) and records neural state in the install record (e.g. `neural_python` path).
- Base path unchanged: no venv → system 3.9, zero deps.

### Model pre-download
After deps install, drive the venv Python to build `KokoroEngine(SONARI_DIR/"kokoro")` and trigger `_ensure_loaded()` once, so the first real utterance isn't a multi-minute download stall. (Distinct from #53's *in-utterance* "downloading…" cue.)

### Verify (eyes-free)
`voices install` ends by confirming: the venv imports kokoro, the daemon restarted on the venv interpreter, the model is present - and reports the result audibly/printed with an actionable message on failure. `sonari doctor` reflects the same.

### Uninstall / revert
`voices uninstall` removes `~/.sonari/venv` (model optional), clears the neural-state record, re-wires the LaunchAgents with the system Python, and restarts.

### Error handling
Every fallible step (uv bootstrap, venv create, dep install, model download, launchctl) either succeeds or leaves the previous working state intact and reports an actionable error - never a daemon wired to a half-built venv.

## Testing strategy
- **Unit (TDD, mocked seams)** - mirror `test_cli_install.py` style (mock uv/launchctl/fs/install-record): interpreter selection (neural→venv, base→3.9), idempotency, re-install keeps the venv, uninstall reverts, step ordering, and error paths (uv missing→bootstrap; provision failure→actionable, no half-wire).
- **Real provisioning verified by dogfooding on this Mac** - run `sonari voices install`, hear a neural voice, `doctor` green. Not in CI (network + heavy).

## Scope / non-goals (follow-ups)
- **Windows neural provisioning** - parity follow-up (Task Scheduler + `venv\Scripts\python.exe`; #42 has the same 3.9-class constraint). Keep selection/provisioning logic platform-neutral where practical; the supervisor backends write the agent.
- **claude-everywhere bootstrap integration** - thin follow-up: the bootstrap calls `sonari voices install`. Native binaries (onnxruntime) are arch-specific and must **not** be file-synced across machines - each machine provisions its own.
- **In-utterance "downloading…" earcon (#53)** and **actionable-by-ear errors (#54)** - separate, already filed.
- **Out-of-process synth helper** - considered and rejected for now (keeps in-process per merged design); revisit only if base speech must be fully isolated from the neural env.
