---
name: sonara-gates
description: Use before committing, opening or recommending a Sonara PR. The gate script packaging/gate.py, the exact gate commands (Rust, notices, conformance, ruff, pytest) and when the e2e and SDK gates also apply.
---

# Sonara gates

All must pass before a commit is called done. Run from the repo root (or the worktree) in Git
Bash or PowerShell, with `~/.cargo/bin` on PATH (`export PATH="$HOME/.cargo/bin:$PATH"`).

## The gate script (#256)

```sh
python packaging/gate.py            # the gates the change needs (vs origin/main plus the working tree)
python packaging/gate.py --dry-run  # print the plan only
python packaging/gate.py --quick    # inner loop: clippy and tests only for the changed crates and their dependents
python packaging/gate.py --all      # every gate but earcons, whatever changed
```

It picks the gates from the changed paths (table below), runs them in CI order, stops at the
first failure (`--keep-going` runs the rest) and prints one line per step. Tests run through
`cargo nextest` when it is installed (`cargo install cargo-nextest --locked`; much faster, as CI
uses it), else `cargo test`. Conformance and e2e run against `target/debug` (it sets `SONARAD`
and `SONARA_HOOK`), so a release build from the SDK gate never shadows them and nothing needs
trashing. Before a PR, run it without `--quick`: a version bump selects every gate but e2e and
earcons.

## The commands it runs

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace && cargo test --workspace --doc   # or: cargo test --workspace
cargo deny check licenses bans
python packaging/notices/gen_notices.py --check
cargo build -p sonarad -p sonara-hook -p sonara-cli
python -m pytest conformance -q
```

Run by hand, conformance picks the newest of `target/release` and `target/debug`
(`conformance/harness.py`): set `SONARAD` and `SONARA_HOOK` to the debug exes, or trash a stale
release `sonarad.exe` first (`trash`, never `rm`).

Python lint and repo checks (Python 3.12, `python -m pip install --group dev`; plain
`python -m pytest -q` also runs `tests/e2e`, which skips without playwright or sonarad):

```sh
ruff check tests conformance clients/python packaging
python packaging/bump_version.py --check
python -m pytest tests/repo -q
```

## Also, when the change touches (the script selects these itself)

| Paths | Extra gate |
|---|---|
| `crates/sonarad/assets/settings.html`, `crates/sonarad/src/settings_page.rs` | `cargo build -p sonarad`, then `python -m pytest tests/e2e -q` (needs `python -m pip install --group e2e`, `python -m playwright install chromium`). CI skips it: it is the local gate. |
| `clients/`, `packaging/npm-runtime`, `packaging/smoke`, `examples/`, any version file (every PR bumps one) | SDK gates below |
| Rust dependencies (`Cargo.toml`, `Cargo.lock` beyond the version) | `python packaging/notices/gen_notices.py`, commit the result |
| `crates/sonara-agent/sounds/`, `packaging/sounds/` | `python packaging/sounds/build_earcons.py --check` |

SDK gates (CI clients job runs Node 18 and Python 3.9):

```sh
cargo build -p sonarad --release
python packaging/runtime_dlls.py stage target/release
(cd clients/ts && npm ci && npm run typecheck && npm run build && npm test)
(cd clients/player && npm ci && npm run typecheck && npm run build && npm test)
(cd examples/player-demo && npm ci && npm run build)
(cd packaging/npm-runtime && npm run build && npm test)
node packaging/smoke/run-node.mjs
python -m pytest clients/python/tests -q
SONARA_RUNTIME=target/release/sonarad.exe PYTHONPATH=clients/python/src python packaging/smoke/python_host.py
```

The SDK step builds `target/release/sonarad.exe` without the other release exes; the gate script
pins conformance to the debug build, a hand run must do the same.

## Version

Every PR bumps the version: `python packaging/bump_version.py <major.minor.patch>` (patch for
fixes, minor for features). The release version is `Cargo.toml` `[workspace.package]`;
`tests/repo/test_manifests.py` and `bump_version.py --check` fail when a file disagrees.

Report each gate as one line (`cargo nextest: ok`, as the script's summary prints them), and give detail only on a failure.
Known failures to tolerate: none.
