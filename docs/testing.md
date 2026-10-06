# Testing, live checks and release chores

The gates every PR runs (`python packaging/gate.py` picks and runs them) are in `CLAUDE.md`, `CONTRIBUTING.md` and the project skill
`.claude/skills/sonara-gates`. This page holds the rest:
opt-in live tests, engine staging, SDK steps, generated assets, the version bump and the
safe redeploy. Rust commands need `~/.cargo/bin` on PATH.

## Rust live tests (opt-in, on this PC)

| What | Command |
|---|---|
| OneCore voices | `cargo test -p sonara-engine --test onecore_live -- --ignored` |
| L4 audio, media and hotkeys | `cargo test -p sonara-system --test win_live -- --ignored` |
| Credential Manager round trip | `cargo test -p sonara-engine --test credman_live -- --ignored` |
| End to end, one sentence | `cargo run -p sonara-reader --example say -- --engine onecore\|fake "Hello."` |

## External engines (#224, #225, #226, #235)

- Conformance and SDK tests run `sonarad --keys fake`: keys live in `<home>\fake-keys.json`,
  never in Credential Manager.
- The `command` kind's tests run `crates/sonara-engine/src/bin/sonara-fake-tts.rs`
  (feature `test-util`).
- Live checks (opt-in, each skips when its variables are unset):
  `cargo test -p sonara-engine --test external_live -- --ignored --nocapture`

| Provider | Variables |
|---|---|
| OpenAI | `OPENAI_API_KEY` + `SONARA_LIVE_OPENAI_VOICE` (it has no voice list) |
| Kokoro-FastAPI | `SONARA_LIVE_KOKORO_FASTAPI_URL` |
| LocalAI | `SONARA_LIVE_LOCALAI_URL` + `SONARA_LIVE_LOCALAI_MODEL` |
| Speaches | `SONARA_LIVE_SPEACHES_URL` + `SONARA_LIVE_SPEACHES_MODEL` |
| ElevenLabs | `ELEVENLABS_API_KEY` |
| Azure | `AZURE_SPEECH_KEY` + `AZURE_SPEECH_REGION` |
| Google Cloud TTS | `GOOGLE_TTS_API_KEY` |
| Gemini | `GEMINI_API_KEY` |
| Cartesia | `CARTESIA_API_KEY` + `SONARA_LIVE_CARTESIA_MODEL` |
| Deepgram | `DEEPGRAM_API_KEY` |
| Your own program | `SONARA_LIVE_COMMAND` (JSON argv), optional `SONARA_LIVE_COMMAND_OPTIONS` (JSON object) |

Any check also takes `SONARA_LIVE_<NAME>_MODEL` and `SONARA_LIVE_<NAME>_VOICE`; without them it
uses the provider's first listed model or voice; a provider without a voice list (OpenAI) needs the
voice variable. No model ids or voice names in code (#235).

## Kokoro engine and G2P

- `python packaging/runtime_dlls.py stage target/<debug|release>` puts `onnxruntime.dll`
  (pinned SHA-256) and the VC++ DLLs it needs next to `sonarad.exe`. Without them sonarad falls
  back to OneCore. The CRT is linked statically (`.cargo/config.toml`).
- Live check:
  `SONARA_KOKORO_MODELS=<folder with the two model files> cargo test -p sonara-engine --release --test kokoro_live -- --ignored --nocapture`
- G2P golden changes: `SONARA_BLESS=1 cargo test -p sonara-engine --test kokoro_g2p`, then
  review the diff.
- Vendored G2P data: `crates/misaki/tools/pack_data.py`.

## Conformance

`cargo build -p sonarad -p sonara-hook -p sonara-cli; python -m pytest conformance -q` runs the
runtime as a black box (`--engine fake --system fake`, temp `SONARA_HOME`) against
`docs/protocol-v1.md`. `conformance/plugin/` drives `bin/` in Git Bash and the PowerShell
bootstrap against a local release server, `sonara.exe`, and a release zip installed into a temp
`LOCALAPPDATA`. A lone stale `target/release/sonarad.exe` can shadow the debug build: `trash` it
first.

## SDKs (CI clients job, Node 18 and Python 3.9)

Run when `clients/`, `packaging/npm-runtime` or a version file changes. Bundling guide:
`docs/bundling.md`.

```sh
cargo build -p sonarad --release
cd clients/ts && npm ci && npm run build && npm test        # test:unit needs no sonarad
cd clients/player && npm ci && npm run typecheck && npm run build && npm test
python -m pytest clients/python/tests -q
cd packaging/npm-runtime && npm run build && npm test
node packaging/smoke/run-node.mjs
```

Player demo: `examples/player-demo` (see `clients/player/README.md`). npm and PyPI publishing is
manual (`docs/bundling.md`).

## Settings page (e2e)

`python -m pytest tests/e2e -q` (needs `python -m pip install --group e2e` and
`python -m playwright install chromium`, plus `cargo build -p sonarad`). Run it when `crates/sonarad/assets/settings.html` or
`crates/sonarad/src/settings_page.rs` change.

## Generated assets

- Earcons (#211): the bundled WAVs are `crates/sonara-agent/sounds/<kind>.wav`, rendered by
  `python packaging/sounds/build_earcons.py` (numpy + scipy; `--check` compares with
  `SHA256SUMS`). Never commit the user's reference recordings.
- Notices (R6): after any Rust dependency change run `python packaging/notices/gen_notices.py`
  (CI runs `--check`). Model and data notices are hand-kept in
  `packaging/notices/models-and-data.md`.

## Version files

Bump in every PR (patch for fixes, minor for features) with
`python packaging/bump_version.py <major.minor.patch>`. Its `VERSION_FILES` table is the one list
of files that carry the version (`Cargo.toml` `[workspace.package]`, the release version since
#248, `bin/runtime-version`, the plugin manifests, the SDK and npm runtime packages); it also moves the
workspace crates in `Cargo.lock` and the package entries in the two client `package-lock.json`
files, keeps line endings, and changes nothing if a file does not match.
`tests/repo/test_manifests.py` reads the same table and checks that every file agrees. A new
version file goes into that table. `python packaging/bump_version.py --check` prints the release
version, or names the files that disagree and exits 1: release.yml reads the version with it,
and the ci.yml check job runs it on every PR as the dry run.

Release: a push to main runs ci.yml; release.yml starts once it passed (`workflow_run`, #250),
builds the runtime zip and publishes `v<version>`. It refuses a version that already exists.

## Safe redeploy of a branch build

The project skill `.claude/skills/sonara-redeploy` has the full procedure, including a new
version folder and the rollback.

The plugin runs the runtime in `%LOCALAPPDATA%\Sonara\runtime\<bin/runtime-version>\`. The short
form, to try a branch build in the same version folder (never during someone's session):

1. `cargo build -p sonarad -p sonara-hook -p sonara-cli --release; python packaging/runtime_dlls.py stage target/release`
2. `"$LOCALAPPDATA/Sonara/runtime/<ver>/sonara.exe" stop` (writes `stopped`, restores ducked
   apps, waits for the exit).
3. Copy `target/release/{sonarad,sonara-hook,sonara}.exe` and the staged DLLs into that folder.
4. `"$LOCALAPPDATA/Sonara/runtime/<ver>/sonara.exe" start` (clears `stopped`).

## Launcher and logs

- `bin/sonara-hook-launch` reads `bin/runtime-version` and execs `sonara-hook.exe` (adds
  `--standalone` to `SONARA_RUNTIME_ARGS`). Without the runtime it starts
  `bin/sonara-bootstrap.ps1` once in the background (lock `runtime\.bootstrap.lock`, retry
  marker `.bootstrap.failed`, 5 min). Test overrides: `SONARA_RELEASE_BASE_URL`,
  `SONARA_BOOTSTRAP_START=0`, `SONARA_NO_BROWSER`.
- Logs in `%LOCALAPPDATA%\Sonara\logs\` (crate `sonara-log`, #219: 1 MB segments, 10 MB for the
  folder, oldest out first): `sonarad.log` (#217: `in` messages, `agent` decisions, `read text`,
  `drop` reasons), `hook.log` (each hook's payload and what it sent), `bootstrap.log`. Text is
  logged only while the setting `debug_log` is on (the default).
- Settings change live through the settings page (`sonara.exe settings`) or protocol `set`;
  product defaults are `sonarad::config::SCHEMA`.
