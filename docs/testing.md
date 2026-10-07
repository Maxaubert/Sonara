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

### Contract tests against the providers' own specs (#275)

`cargo test -p sonara-engine --test 'contract_*'` (part of `cargo test --workspace`, no network).
Each external kind and preset has a schema fragment taken from its provider's official spec in
`crates/sonara-engine/tests/contracts/<provider>/fragment.json` (OpenAI, ElevenLabs, Deepgram,
LocalAI and Speaches OpenAPI; Google Cloud TTS and Gemini discovery documents; google.rpc.Status;
hand-transcribed from docs or server source for Azure, Cartesia, Kokoro-FastAPI, the two Chatterbox
servers and openedai-speech). Each folder's `SOURCES.md` gives the URLs, the date, the commit and
the licence. The tests capture each adapter's real request (method, path, query, headers, body)
and validate it against the fragment (request bodies strictly: a field the spec does not name
fails), and feed spec-shaped answers (audio, voice and model lists, error bodies, each validated
against the fragment first) through the adapter's parsing and error mapping.

To refresh a fragment: fetch the spec into the session scratchpad, run the command in its
`SOURCES.md` (`python packaging/contracts/extract_fragment.py ...`), update the date and commit
there, and run the tests: a failure is either a spec change Sonara must follow or a bug.
The command does not rebuild everything: re-apply the edits a `SOURCES.md` lists under
"Hand-added" and "Derived" (for example Gemini's `voices.list` and Speaches'
`GET /v1/audio/voices`), or the tests fail as if the spec had changed.

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
python packaging/build_packages.py --publish-dry-run          # the packages release.yml publishes
```

Player demo: `examples/player-demo` (see `clients/player/README.md`). npm and PyPI publishing is
automatic once switched on (trusted publishing, #279; setup in `docs/bundling.md`).

## Embedder e2e (`tests/embed`, #274)

Apps that use Sonara for speech, built the way a developer would, against the release runtime:

| Host | What it proves |
|---|---|
| Node (`test_node_host.py`) | `@sonara/client` and `@sonara/runtime-win32-x64` packed as published and installed into an empty project (ESM, CommonJS, types, `bin/` with the DLLs); the whole public API (autostart, speak append/replace/interrupt, pause/resume/skip/stop, mute, volume, rate, voice, state and item events, channels open/speak/next/close, engines list/add/test/voices/remove against a local fake provider, `set engine`); real Kokoro speech from `node_modules` |
| Python (`test_python_host.py`) | the `sonara-client` wheel (built from a copy of `clients/python`) in a fresh venv (`uv venv`, else `python -m venv`); the same scenario with `sonarad.exe` from a release zip that `packaging/release_zip.py` builds and the test checks against `SHA256SUMS`; real Kokoro speech from the zip |
| Raw protocol (`test_raw_host.py`, `raw_client.py`) | a client written only from `docs/protocol-v1.md` and `docs/bundling.md`: discovery, TCP JSON lines, HTTP status codes, SSE, errors, channels, takeover, idle exit; the doc examples (protocol-v1 Python and curl, the bundling.md PowerShell recipe in PowerShell 7 and 5.1) run as written |
| Released zip (`test_release_zip.py`, opt-in) | `SONARA_EMBED_RELEASE=1`: `gh release download` of the latest release (`SONARA_EMBED_RELEASE_TAG` for another), checked and unpacked, the Node host run against it |

```sh
cargo build -p sonarad -p sonara-hook -p sonara-cli --release
python packaging/runtime_dlls.py stage target/release
(cd clients/ts && npm ci)
python -m pytest tests/embed -q -rs        # SONARAD names another runtime
SONARA_EMBED_RELEASE=1 python -m pytest tests/embed/test_release_zip.py -q -rs
```

The audio is checked, not only the events: every run starts the runtime with
`--output wav:<dir>` (a testing aid, see `docs/protocol-v1.md` "Testing aids"), which keeps time
like `--output null` and writes each chunk it is handed to `<seq>-item<id>-chunk<n>.wav` (clips:
`<seq>-clip.wav`). The fake engine's tone has an exact length per character, so the tests check
that the rate and voice reached the engine, that skipped items never played, and that an external
engine's audio (a constant the fake provider sends) is what played, not the fallback. The
real-voice tests check speech: 24 kHz for Kokoro, 1.2 to 10 s for the test sentence, peak, RMS,
pauses and many distinct levels. They need `onnxruntime.dll` next to the runtime and Kokoro's
model (`SONARA_KOKORO_MODELS`, else the one in `%LOCALAPPDATA%\Sonara\models\kokoro\v1.0`, only
read: the temp home gets hard links); without them the Kokoro test skips. With them, Kokoro
reporting `unavailable` is a failure (the bundle's DLLs did not load), and a negative test runs
the zip's folder without `onnxruntime.dll` and expects exactly that. The engine status after the
item must still name the same engine, ready and with no fallback. The OneCore test skips
when the PC has no usable Windows voice (`engine_status` `unavailable`). Every run has its own temp
`SONARA_HOME`; nothing touches the user's runtime. `python packaging/gate.py` runs the suite as
the `embed` gate; CI runs it in the `clients` job (no Kokoro model there).

## Settings page (e2e)

`python -m pytest tests/e2e -q` (needs `python -m pip install --group e2e` and
`python -m playwright install chromium`, plus `cargo build -p sonarad`). Run it when `crates/sonarad/assets/settings/`, `crates/sonarad/src/config.rs` or
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
- Re-check on a Claude Code upgrade (#283): the hook reads a question's thinking from the
  transcript only when its message has no text block, because `MessageDisplay` payloads carry no
  thinking (2026-10-07: 972 payloads in `hook.log`, fields `cwd, delta, final, index, message_id,
  prompt_id, scratchpad_dir, session_id, transcript_path, turn_id, hook_event_name`). If a later
  Claude Code streams thinking through `MessageDisplay`, a thinking-only question would be read
  twice (as prose and as its lead-in): drop the lead-in (`sonara-hook` `transcript`) then. The
  transcript row shape the reader expects (one row per content block, rows of a message sharing
  `message.id`) is in `tests/fixtures/transcripts/`.
- Settings change live through the settings page (`sonara.exe settings`) or protocol `set`;
  product defaults are `sonarad::config::SCHEMA`.
