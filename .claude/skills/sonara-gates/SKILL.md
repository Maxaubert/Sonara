---
name: sonara-gates
description: Use before committing, opening or recommending a Sonara PR. The exact gate commands (Rust, notices, conformance, ruff, pytest) and when the e2e and SDK gates also apply.
---

# Sonara gates

All must pass before a commit is called done. Run from the repo root (or the worktree) in Git
Bash, with `~/.cargo/bin` on PATH (`export PATH="$HOME/.cargo/bin:$PATH"`).

## Always

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses bans
python packaging/notices/gen_notices.py --check
```

Conformance runs the binaries as a black box and picks the newest of `target/release` and
`target/debug` (`conformance/harness.py`). A stale or lone `target/release/sonarad.exe` (the SDK
step builds only that one) then shadows the fresh debug build, so trash the release exes first
(`trash`, never `rm`):

```sh
for f in sonarad sonara-hook sonara; do [ -f target/release/$f.exe ] && trash "$PWD/target/release/$f.exe"; done
cargo build -p sonarad -p sonara-hook -p sonara-cli
python -m pytest conformance -q
```

Python lint and repo checks (system Python with `.[dev,windows]`):

```sh
ruff check src tests conformance clients/python packaging
python -m pytest -q
```

## Also, when the change touches

| Paths | Extra gate |
|---|---|
| `crates/sonarad/assets/settings.html`, `crates/sonarad/src/settings_page.rs` | `cargo build -p sonarad`, then `python -m pytest tests/e2e -q` (needs `pip install -e ".[e2e]"`, `playwright install chromium`). CI skips it: it is the local gate. |
| `clients/`, `packaging/npm-runtime`, any version file (every PR bumps one) | SDK gates below |
| Rust dependencies (`Cargo.toml`, `Cargo.lock` beyond the version) | `python packaging/notices/gen_notices.py`, commit the result |
| `crates/sonara-agent/sounds/`, `packaging/sounds/` | `python packaging/sounds/build_earcons.py --check` |

SDK gates (CI clients job runs Node 18 and Python 3.9):

```sh
cargo build -p sonarad --release
(cd clients/ts && npm ci && npm run typecheck && npm run build && npm test)
(cd clients/player && npm ci && npm run typecheck && npm run build && npm test)
python -m pytest clients/python/tests -q
(cd packaging/npm-runtime && npm run build && npm test)
node packaging/smoke/run-node.mjs
```

The SDK step builds `target/release/sonarad.exe` without the other release exes: trash it again
before the next conformance run.

## Version

Every PR bumps the version: `python packaging/bump_version.py <major.minor.patch>` (patch for
fixes, minor for features). `tests/test_manifests.py` fails when a file disagrees.

Report each gate as one line (`cargo test --workspace: ok`), and give detail only on a failure.
Known failures to tolerate: none.
