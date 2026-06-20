# Contributing to Sonara

Sonara is maintained by two people on two operating systems (macOS and Windows).
The whole workflow is built around one fact: **neither maintainer can verify the
other's platform.** Machines check the portable logic; humans check their own
platform's runtime.

## Branch model

- **`main` is the trunk and is always releasable.** There are no long-lived
  integration branches.
- **Branch off `main`, one concern per branch, short-lived.** Name it
  `area/short-desc`:
  - `win/...` — Windows backend (`src/sonara/platform/windows/**`)
  - `macos/...` — macOS backend (`src/sonara/platform/macos/**`)
  - `core/...` — shared core (daemon, assembler, speaker, protocol, keymap)
  - `docs/...`, `test/...` — docs and test-only changes
- **One concern per PR.** If a change spans three layers (e.g. a port + a UX
  feature + a review pass), open three PRs, not one. Big multi-layer branches are
  hard to review and hard to trace.
- **Squash-merge into `main`** — every PR becomes a single commit on `main`.
  Commit however you like *inside* your branch; history is squashed at merge.
- **Delete the branch after it merges** (locally and on the remote).

## Ownership

| Area | Owner | Review rule |
|------|-------|-------------|
| `src/sonara/platform/windows/**` | Max | Max approves; Nima reviews for design |
| `src/sonara/platform/macos/**` | Nima | Nima approves; Max reviews for design |
| Shared core + everything else | both | **both** approve |

The owner of a platform is the only person who can sign off that platform's
**runtime** behavior — see below.

## Two layers of verification

**1. The logic suite (machine-checkable, runs anywhere).**
The `pytest` suite uses fakes for audio and hotkeys, so it runs headless on any
OS. Before opening a PR, run it on your platform:

```
python -m venv .venv && .venv/bin/pip install -e ".[dev]"
.venv/bin/python -m pytest -q
```

It must be green on your platform. CI will run it on **both** macOS and Windows
on every PR (coming soon); until then, the cross-platform owner confirms it on
the other OS as part of review. A test that can only run on one OS must be
`skipif`-guarded for the other (see `test_win_supervisor.py`), never left to fail.

**2. Runtime acceptance (human, per-platform).**
The suite proves *nothing* about real speech, the daemon crash/interrupt path, the
global-hotkey pump, earcon mixing, or autostart — those are OS-runtime behaviors
only a real machine can confirm. **Each owner runs their own platform's
acceptance checklist on real hardware and signs off before merge.** A change that
alters runtime behavior on the other OS cannot merge until that OS's owner has
accepted it on hardware.

## Platform discipline

The platform seam (`src/sonara/platform/`) keeps OS-specific code isolated. **No
Windows-only import may appear on any shared or macOS code path, and vice versa.**
This is what lets each owner trust that the other's platform work can't regress
their own.

## A PR merges when

1. It is one concern, branched off `main`.
2. The logic suite is green on the author's platform (and, soon, in CI on both).
3. At least one required owner has approved.
4. If it touches runtime behavior on an OS, that OS's owner has accepted it on
   real hardware.
5. It is squash-merged, and the branch is deleted.

## Behavior changes

Sonara is an eyes-free tool — changes to core controls (hotkeys, what gets
spoken, default bindings) are user-facing decisions. **Call them out explicitly**
in the PR description (a `⚠️ behavior change` line) rather than burying them in a
feature branch, and raise anything that removes or remaps a default before you
build it.
