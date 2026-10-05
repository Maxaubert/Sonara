# Contributing to Sonara

Sonara is a Windows-only tool. Verification has two layers: the gates check the logic, and a
human checks the Windows runtime on real hardware. Start with [docs/README.md](docs/README.md)
for where everything lives, and [docs/architecture.md](docs/architecture.md) for how the code
fits together.

## Branch model

- **`main` is the trunk and is always releasable.** There are no long-lived integration
  branches.
- **One issue, one branch, one PR.** Branch off `main` as `type/<issue>-slug`
  (`fix/128-up-always-restart`, `feat/...`, `refactor/...`, `docs/...`, `ci/...`).
  Commits are `type(scope): subject (#issue)`. A version bump or tooling change needed by a
  change rides inside that change's PR.
- **Bump the version in every PR**: patch for fixes, minor for features, with
  `python packaging/bump_version.py <version>` (it sets every version file and lockfile entry;
  `tests/repo/test_manifests.py` keeps them equal; the release version is `Cargo.toml` `[workspace.package]`). A push to `main` runs CI, and once it passes `release.yml` publishes
  `v<version>`; it refuses a version that already exists.
- **Squash-merge into `main`**, then delete the branch locally and on the remote.

## 1. The gates (run anywhere)

The runtime is the Rust workspace in `crates/`. Tests use fakes for speech, audio and hotkeys
(`--engine fake --system fake`), so they need no speech engine and run headless. Install Rust
with rustup (`rust-toolchain.toml` picks stable with clippy and rustfmt), Python 3.12 or
newer (pip 25.1 or newer), and `cargo install cargo-deny`. Before opening a PR:

```powershell
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses bans
python packaging/notices/gen_notices.py --check

# Protocol v1 conformance (black box against the built binaries)
cargo build -p sonarad -p sonara-hook -p sonara-cli
python -m pytest conformance -q

# Python lint and repo checks (pyproject.toml holds only dev dependency groups)
python -m venv .venv
.venv\Scripts\python -m pip install --upgrade pip
.venv\Scripts\python -m pip install --group dev
.venv\Scripts\ruff check tests conformance clients/python packaging
.venv\Scripts\python packaging/bump_version.py --check
.venv\Scripts\python -m pytest -q
```

All must be green. CI (`.github/workflows/ci.yml`) runs the same on Windows, plus the SDK
clients job. Tests must not depend on what is installed on your PC (Kokoro models, Windows
voices, API keys).

- **Settings page changes** (`crates/sonarad/assets/settings.html`,
  `crates/sonarad/src/settings_page.rs`) also need the browser tests:
  `python -m pip install --group e2e`, `python -m playwright install chromium`, `cargo build -p sonarad`, then
  `python -m pytest tests/e2e -q`. CI skips them, so they are the local gate.
- **SDK changes** (`clients/`, `packaging/npm-runtime`, version files) run the SDK steps in
  [docs/testing.md](docs/testing.md).
- **Real speech, audio and hotkey checks** are opt-in `--ignored` tests, listed with their
  env vars in [docs/testing.md](docs/testing.md).
- **Bug fixes are test-first**: a regression test named after the behaviour, failing before the
  fix.

## 2. Runtime acceptance (a human, on Windows)

The gates prove nothing about real speech, the runtime's crash and restart paths, the global
hotkeys, earcon mixing or ducking. For a change that touches runtime behaviour, deploy the
branch build into `%LOCALAPPDATA%\Sonara\runtime\<version>\` (the safe redeploy steps are in
[docs/testing.md](docs/testing.md)), use it in a real Claude Code session, and say in the PR
what you tested.

## Code rules

- Crates are layered (L1 core to L5 hook and CLI, `sonarad` on top) and never depend upward;
  `crates/sonara-core/tests/layering.rs` (one table of allowed edges) enforces it.
- Protocol changes are additive and update [docs/protocol-v1.md](docs/protocol-v1.md) (or
  [docs/protocol-v1-engines.md](docs/protocol-v1-engines.md) for external engines), the
  conformance tests and both SDKs.
- A new file in `%LOCALAPPDATA%\Sonara` is listed in [PRIVACY.md](PRIVACY.md).
- No em-dashes anywhere.

## A PR merges when

1. It is one concern, branched off `main`.
2. The gates are green, locally and in CI.
3. A maintainer has approved it.
4. If it touches runtime behaviour, it has been tested on real hardware.

## Behaviour changes

Sonara is an eyes-free tool, so changes to core controls (hotkeys, what gets spoken, default
bindings) are user-facing decisions. Call them out in the PR description with a
**Behaviour change:** line, and raise anything that removes or remaps a default before you
build it.
