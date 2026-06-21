# Contributing to Sonara

Sonara is a Windows-only tool. The workflow separates the two kinds of
verification: machines check the portable logic; humans check the Windows
runtime on real hardware.

## Branch model

- **`main` is the trunk and is always releasable.** There are no long-lived
  integration branches.
- **Branch off `main`, one concern per branch, short-lived.** Name it
  `area/short-desc`:
  - `win/...` — Windows backend (`src/sonara/platform/windows/**`)
  - `core/...` — shared core (daemon, assembler, speaker, protocol, keymap)
  - `docs/...`, `test/...` — docs and test-only changes
- **One concern per PR.** If a change spans three layers (e.g. a port + a UX
  feature + a review pass), open three PRs, not one. Big multi-layer branches are
  hard to review and hard to trace.
- **Squash-merge into `main`** — every PR becomes a single commit on `main`.
  Commit however you like *inside* your branch; history is squashed at merge.
- **Delete the branch after it merges** (locally and on the remote).

## Two layers of verification

**1. The logic suite (machine-checkable, runs anywhere).**
The `pytest` suite uses fakes for audio and hotkeys, so it runs headless on any
OS, including a non-Windows dev box or CI. Before opening a PR, run it:

```
python -m venv .venv && .venv/bin/pip install -e ".[dev]"
.venv/bin/python -m pytest -q
```

It must be green. A test that can only run on Windows must be `skipif`-guarded
for other platforms (see `test_win_supervisor.py`), never left to fail.

**2. Runtime acceptance (human, on Windows).**
The suite proves *nothing* about real speech, the daemon crash/interrupt path, the
global-hotkey pump, earcon mixing, or autostart — those are OS-runtime behaviors
only a real machine can confirm. **Run the Windows acceptance checklist on real
hardware and sign off before merge.** A change that alters runtime behavior
cannot merge until it has been accepted on hardware.

## Platform discipline

The platform seam (`src/sonara/platform/`) keeps OS-specific code isolated, so
the portable core stays free of Windows-only imports and remains testable on any
OS.

## A PR merges when

1. It is one concern, branched off `main`.
2. The logic suite is green (and, soon, in CI).
3. At least one maintainer has approved.
4. If it touches runtime behavior, it has been accepted on real hardware.
5. It is squash-merged, and the branch is deleted.

## Behavior changes

Sonara is an eyes-free tool — changes to core controls (hotkeys, what gets
spoken, default bindings) are user-facing decisions. **Call them out explicitly**
in the PR description (a `⚠️ behavior change` line) rather than burying them in a
feature branch, and raise anything that removes or remaps a default before you
build it.
