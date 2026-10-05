# Sonara architecture

How the Rust runtime in `crates/` is put together, for contributors. User-facing behaviour is in
the [README](../README.md); the wire contract for clients is [protocol-v1.md](protocol-v1.md)
(external engines: [protocol-v1-engines.md](protocol-v1-engines.md)). The design came from the
runtime spec and plan in `docs/plans/`. The retired Python daemon's architecture (up to 0.10) is
kept in [history/architecture-python.md](history/architecture-python.md).

Each crate's `src/lib.rs` opens with a `//!` header that says what it owns and the rules it keeps.
This page is the map; the headers are the detail.

## Contents

- [Layers and crates](#layers-and-crates)
- [Process chain](#process-chain)
- [Inside sonarad](#inside-sonarad)
- [Threads](#threads)
- [Locks and the lock order](#locks-and-the-lock-order)
- [External engines](#external-engines)
- [Persisted state and logs](#persisted-state-and-logs)
- [How to](#how-to)

## Layers and crates

Five layers, each building only on the ones below it; `sonarad` hosts them all. The rule (R7 in the
runtime spec) is checked by `crates/sonara-core/tests/layering.rs`: one table of the workspace
crates each crate may depend on, read against `cargo metadata`, so a new dependency on a higher layer
(or a new crate without a row) fails a test.

| Crate | Layer | Owns | Workspace deps | Key files | Tests |
|---|---|---|---|---|---|
| `sonara-core` | L1 | Pure text rules (markdown cleanup, the streamed-text assembler) and the reader state machine (one queue of items, controls, state and item events). No threads, no I/O, no clock | none | `text.rs`, `assembler.rs`, `reader/mod.rs`, `reader/types.rs`, `reader/chunks.rs` | `tests/golden.rs` (shared fixtures in `tests/fixtures/text_rules/`), `tests/reader_*.rs`, `tests/layering.rs` (R7 guard for the whole workspace) |
| `sonara-engine` | L1 | The `Engine` trait, its types, the `Registry` (licence rule R6), OneCore, Kokoro (feature `kokoro`), external engines (feature `external`), the fake engine (feature `test-util`) | `sonara-log` (feature `external`), `misaki` (feature `kokoro`) | `lib.rs`, `types.rs`, `registry.rs`, `onecore/`, `kokoro/`, `external/`, `http.rs`, `bin/sonara-fake-tts.rs` | `tests/registry.rs`, `tests/fake.rs`, `tests/kokoro_*.rs` (G2P goldens in `tests/golden/`), `tests/external_*.rs`; opt-in live: `onecore_live`, `kokoro_live`, `external_live`, `credman_live` |
| `misaki` | L1 | Vendored, trimmed misaki G2P (US English) for Kokoro | none | `g2p.rs`, `lexicon.rs`, `tagger.rs`, `data/` (packed by `tools/pack_data.py`) | `tests/data.rs` |
| `sonara-audio` | L1 | Audio output: plays one chunk's PCM with true pause and resume, volume, clips mixed over speech; device trouble is an `AudioEvent::Failed`, never a panic | `sonara-core`, `sonara-engine` | `rodio_output.rs` (rodio on WASAPI, own thread), `test_output.rs` (feature `test-util`) | unit tests |
| `sonara-reader` | L1 | The facade `ReaderHandle`: a worker thread owns the state machine and carries out its effects with an engine and an output; synthesis on its own thread; `subscribe` for events | `sonara-core`, `sonara-engine`, `sonara-audio` | `lib.rs`, `worker.rs`, `synth.rs`, `settings.rs` | `tests/playback.rs`, `threads.rs`, `lookahead.rs`, `settings.rs`, `engine_status.rs` |
| `sonara-log` | L1 | The log folder: streams, 1 MB segments, a 10 MB budget for the folder, an OS lock per append; secret masking; `scrub` and `FIELD_MAX` for JSON written to the log (sonarad and the hook) | none | `lib.rs`, `secrets.rs`, `json.rs` | `tests/budget.rs`, unit tests (`json.rs`, `secrets.rs`) |
| `sonara-channels` | L2 | Several named sources share one reader; one channel reads at a time, switches are announced, entries are fed one at a time when the reader is idle | `sonara-reader` | `lib.rs` (the driver), `router.rs` (pure rules) | `tests/channels.rs`, `router.rs` |
| `sonara-agent` | L3 | Agent sessions on channels: streamed turns, decisions with priority, earcons (one at a time), three mute levels, read modes, summaries | `sonara-channels`, `sonara-reader`, `sonara-core`, `sonara-engine` | `lib.rs` (the driver), `rules.rs` (pure), `decision.rs`, `earcon.rs` + `sounds/`, `sequencer.rs`, `settings.rs`, `summarizer.rs`, `prompts/` | `tests/agent.rs`, `rules.rs`, `muted_store.rs`, `send_mode.rs`, `summarizer_process.rs` |
| `sonara-system` | L4 | Windows extras: duck or pause other apps while speech plays (with crash restore), global hotkeys, the keymap and AltGr check, activity log lines; all OS access behind `platform::Platform` | `sonara-reader` | `audio.rs`, `ducking.rs`, `pausing.rs`, `hotkeys.rs`, `keymap.rs`, `platform.rs`, `win.rs`, `fake.rs`, `log.rs` | `tests/audio.rs`, `ducking.rs`, `pausing.rs`, `hotkeys.rs`, `keymap.rs`, `activity_log.rs`; opt-in live: `win_live` |
| `sonara-client` | L5 support (leaf) | The protocol v1 client of a local `sonarad` (#255): the home (`SONARA_HOME`, else `%LOCALAPPDATA%\Sonara`), `runtime.json` and the stop sentinel, connect, `hello`, a fire-and-forget batch that starts the runtime when none answers (`deliver`), request and reply with events (`Conn`, `attach`), the runtime started detached (`start_runtime`). Only `serde_json` | none | `home.rs`, `runtime.rs`, `hello.rs`, `batch.rs`, `conn.rs` | unit tests; the hook's `tests/binary.rs` and conformance end to end |
| `sonara-hook` | L5 | `sonara-hook.exe`: maps a Claude Code hook event to protocol v1 `channels` and `agent` messages (only the mapping, its `hello` and its log; `sonara-client` delivers them as one batch and starts `sonarad` when none answers); never fails the session | `sonara-client`, `sonara-log` (no runtime crate) | `lib.rs` (`map_event`, `HELLO`, `log_line`), `project.rs` (session names), `main.rs` | `tests/golden.rs` (cases in `tests/golden/`), `binary.rs` |
| `sonara-cli` | L5 | `sonara.exe`: `start`, `stop`, `settings`, `doctor`, `uninstall`, `engines`, `version`; a protocol v1 client through `sonara-client` | `sonara-client` | `main.rs`, `client.rs` (its `hello`), `lifecycle.rs`, `doctor.rs`, `uninstall.rs`, `engines.rs`, `engines_file.rs`, `paths.rs` | unit tests; conformance `conformance/plugin/` |
| `sonarad` | host | The runtime process: one reader, protocol v1 over TCP and HTTP, the extensions, persisted settings, external engine profiles, the settings page, the logs | `sonara-reader`, `sonara-engine`, `sonara-audio`, `sonara-channels`, `sonara-agent`, `sonara-system`, `sonara-log`, `sonara-client` (the home and the `runtime.json` name, shared with the clients) | see [Inside sonarad](#inside-sonarad) | `tests/*.rs`, unit tests, and the black-box `conformance/` suite |

Pure rules and drivers: L1 (`sonara_core::reader`), L2 (`router.rs`) and L3 (`rules.rs`) keep their
decisions in pure code with no threads, clock or I/O, and a driver carries out the actions they
return. New behaviour goes into the rules with a unit test; the driver only executes.

## Process chain

Claude Code hook events:

```
Claude Code hook event
  -> hooks/hooks.json
  -> bin/sonara-hook-launch    Git Bash: reads bin/runtime-version, picks the runtime folder in
                               %LOCALAPPDATA%\Sonara\runtime\<version>\ (bin/sonara-runtime.sh),
                               else starts bin/sonara-bootstrap.ps1 once in the background
  -> sonara-hook.exe <Event>   payload on stdin -> protocol v1 messages (map_event, pure);
                               finds sonarad through <home>\runtime.json, starts it when none
                               answers (within START_BUDGET), sends one batch, exits 0
  -> sonarad.exe               TCP JSON lines, token from runtime.json
```

Slash commands:

```
/sonara:<command> -> commands/<command>.md -> bin/sonara (installs the runtime first when it is
missing) -> sonara.exe <command> -> sonarad.exe (protocol v1, like any client)
```

Other clients (apps that bundle Sonara, the SDKs in `clients/`, the settings page) talk to the same
`sonarad` over TCP or HTTP. Discovery, authentication and lifetime (one instance per user and
home, idle exit, takeover) are in [protocol-v1.md](protocol-v1.md#discovery).

## Inside sonarad

`main.rs` parses the command line (`args.rs`), resolves the home (`home.rs`), takes the
single-instance mutex (`instance.rs`), loads `config.json` (`config.rs`, migrating from the Python
plugin once, `migrate.rs`), builds the engines and the `ReaderHandle`, writes `runtime.json`
(`runtime_file.rs`) and serves on a tokio runtime.

| Module | Role |
|---|---|
| `tcp.rs`, `http.rs` | Framing only: JSON lines; `POST /v1/<type>`, SSE `GET /v1/events`, `GET /settings`. Each request runs `Server::handle` on a blocking thread |
| `protocol.rs` | `Server`: synchronous, transport-free dispatch of core messages and the extensions (`dispatch`), `CAPABILITIES`, `EXTENSION_TYPES`, `EXTENSION_KEYS`, the admission lock, `set`/`get` |
| `wire.rs` | JSON shapes of replies, errors and events |
| `channels_ext.rs`, `agent_ext.rs`, `system_ext.rs` | The extensions on top of L2, L3 and L4: their `TYPES` and `KEYS`, enabled at a client's `hello` and kept for the life of the process |
| `engines.rs`, `engines_ext.rs` | External engine profiles (`engines.json`, keys, registration) and the `engine_*` handlers |
| `events.rs` | Relays reader, earcon and cue events to one client through a bounded queue |
| `cues.rs` | Spoken control cues ("Paused.", "Rate 250.") on one worker, mixed over speech as clips |
| `quiet.rs` | Muted means no request reaches an external engine (#227) |
| `config.rs` | `SCHEMA` of persisted settings, `Store` (`config.json`, `session_prefs.json`), `apply_reader` |
| `lifetime.rs` | Clients, activity and the idle exit |
| `settings_page.rs`, `assets/settings.html` | The settings page, driving the runtime only through the public HTTP API |
| `support_log.rs`, `trace_log.rs` | The lines of `logs\sonarad.log` |
| `null_output.rs` | `--output null`: a silent output that keeps real time (conformance, CI) |

## Threads

| Thread (name) | Crate | What it does |
|---|---|---|
| tokio runtime workers | `sonarad` | Accept connections, frame requests (`tcp.rs`, `http.rs`), the lifetime monitor |
| blocking pool | `sonarad` | `Server::handle` for each request, so a slow handler never stalls the transport |
| `sonara-reader` | reader | Owns the state machine; requests, audio events and finished syntheses arrive on one inbox |
| `sonara-synth` | reader | Engine calls in order, off the control path; cancels the job of an item that ended |
| `sonara-audio-events` | reader | Forwards the output's events into the worker's inbox |
| `sonara-audio` | audio | rodio output (one `Sink` per chunk); `sonarad-null-output` in its place with `--output null` |
| `sonara-channels` | channels | Follows reader events and feeds the next entry when an item ends |
| `sonara-agent-timer` | agent | One per rules timer (settle, hold caps); holds the agent weakly |
| `sonara-summary` | agent | One per summary job (`claude -p` or `codex exec` child process) |
| `sonara-earcons` | agent | Plays earcons that wait behind another one (#238) |
| `sonara-system-audio` | system | Duck, pause, restore: the Core Audio and GSMTC calls |
| `sonara-system-events` | system | Follows the reader's state and tells the audio worker to engage or restore |
| `sonara-hotkeys` | system | RegisterHotKey and the message loop (they must share a thread) |
| `sonara-hotkey-actions` | system | Hands each press to the host, so a busy host never stalls hotkey capture |
| `sonarad-events`, `sonarad-earcons`, `sonarad-cues-relay` | `sonarad` | One set per subscription: drains reader, earcon and cue events into the client's queue |
| `sonarad-cues` | `sonarad` | The cue worker |
| `sonarad-support-log`, `sonarad-cue-log`, `sonarad-read-log` | `sonarad` | Write engine readiness, cue and read-text lines to the log |
| `sonara-kokoro-prepare` | engine | Kokoro model download and load |
| `sonara-external-request`, `sonara-external-stream` | engine | One HTTP request (a wait that `cancel` ends), and a streamed answer's server-sent events |
| unnamed helper threads | agent, engine | Child-process stdin and stdout pumps (summarizer, `command` engine), the sse helpers, the agent's fallback when a summary cannot start; they hold nothing |

Rules:

- The reader worker never waits for an engine, a client or a subscriber: synthesis is on its own
  thread and event channels are unbounded on the reader's side; `sonarad` bounds or drops per
  client (`events::QUEUE`).
- Timer, summary and earcon threads hold the agent weakly and end with it. A thread that finds its
  owner gone exits.
- A slow job (synthesis, a network request, a child process, a file write that can wait) never
  runs while holding a layer lock. Collect what is needed under the lock, release it, then work.

## Locks and the lock order

Each layer's driver has one main lock, so messages apply in the order they arrive:

| Lock | Where | Guards |
|---|---|---|
| agent `rules` | `sonara_agent::Inner::rules` | The pure rules; every agent message runs under it |
| agent `seen` | `sonara_agent::Inner::seen` | The dead-session sweep; taken only while `rules` is held, and it calls into L2 |
| channels `state` | `sonara_channels::Inner::state` | The router, the fed item, the announce and drop hooks, the drops waiting to be reported |
| agent `schedule` | `sonara_agent::Inner::schedule` | Earcons waiting or playing (#238); taken by the session-change chime under the channels' lock |
| agent `player` | `sonara_agent::Inner::player` | The earcon thread's inbox |
| reader | `ReaderHandle` calls | Not a mutex: a call waits for the worker's answer. The worker never calls up into L2 or L3 and takes none of their locks; its events reach other threads through channels |

**Lock order: agent rules > seen > channels state > schedule > player > reader.** Code may take a
lock to the right while holding one to the left, never the reverse. The agent's `trace` and
`subscribers` locks are leaves: held only to clone the hook or to send, never while another lock is
taken. Each crate's header repeats its part (`sonara_channels`, `sonara_agent`,
`ReaderHandle::call`).

Hooks (#255):

- L2 `on_drop` runs **off** the channels' lock: the drops are collected under it and reported, in
  order, right after it is released, on the thread that dropped them. The `before` callback of
  `flush_with` is replayed among them, so each flush line still comes right before its drops. A
  drop hook may call back into `Channels` (`tests/channels.rs` checks it from two threads).
- L2 `on_announce` runs under the channels' lock, because the chime must be queued and the
  announcement held (`hold_start`) before the announcement is fed. L3's hook there takes only
  locks to the right (`schedule`, `player`, the reader) and must not call back into `Channels`.
- L3 `on_trace` runs under the agent's `rules` lock and must not call back into the agent. It stays
  there on purpose: `sonarad` writes trace lines and L2's drop lines into one log, and a wipe line
  must come before the drops it explains; deferring traces past `rules` would put the drops first.
  The `sonarad` hook only formats a line and appends it (a short write under the log's OS lock).

Host locks in `sonarad` are taken at the start of handling a request, before any layer lock:

- `retiring` (the admission lock, `Server::admit`): held while `speak`, `control` and the
  extension messages reach the reader, so the idle exit or a takeover cannot drop an accepted
  request (#194).
- `setting`: one `set` at a time, the change and its record in `config.json` together. Under it,
  `Quiet::change` serializes mute changes.
- `enabling`: one extension enabled at a time.
- `SystemExt::transition` then `holds`: arming and disarming the `system` extension.
- Leaf locks, taken last and held briefly: the `Store` (`config.json` writes), `engines.rs`
  registries and entries, the engine name, `Origins`, the cue queue, the log folder's OS lock.

## External engines

Speech engines the user adds at run time (#224 to #227, #235; spec
`docs/plans/2026-10-04-external-engines-spec.md`; contract
[protocol-v1-engines.md](protocol-v1-engines.md)). Each profile is an `Engine` of licence class
`External` in the reader's `Registry` (shared, with interior mutability, so profiles come and go
while the reader runs), next to Kokoro and OneCore. The reader and the layers above only see
engines. No model id or voice name is in the code: they come from the profile and the provider's
lists.

- `crates/sonara-engine/src/external/` (feature `external`):
  - `profile.rs`: `Kind`, validation, presets, default addresses, options per kind.
  - `mod.rs`: the `External` engine over a `Backend` (an `Adapter`, or a program): fallback with
    the cue, retries, status, the voice rule's last step (`voice_for`), model and voice lists.
  - `adapter.rs`: the `Adapter` trait (one HTTP request per part of a chunk), `execute`, error-body
    shapes, paged lists, stream hooks. One file per kind implements it: `openai.rs`
    (`openai-compatible`), `elevenlabs.rs`, `azure.rs`, `google.rs`, `gemini.rs`, `cartesia.rs`,
    `deepgram.rs`. `command.rs` runs the user's program with no shell instead.
  - `keys.rs` (`Secret`, `KeyStore`: Credential Manager, memory, the `--keys fake` file;
    `KeyResolver`), `health.rs` (breaker, blocked state, the once-per-episode cue; injectable
    clock), `hold.rs` (muted: nothing is sent), `error.rs` (`ExtError`, the cue texts), `cache.rs`
    (the cue cache), `audio.rs` (body to PCM), `rate.rs`, `split.rs`, `worker.rs` (a request on
    its own thread, a wait `cancel` ends), `sse.rs` (a request whose server-sent events arrive on
    a channel), `streaming.rs` (a streamed chunk returns at its first audio within
    `first_audio_ms`, the rest follows on the `PcmStream`; a stall or break after audio reads the
    rest of the message with the fallback from the sentence reached, `rest_of`).
  - Kind details: `azure.rs` builds SSML (XML escaping); `google.rs` decodes a base64
    `audioContent`; `gemini.rs` streams `streamGenerateContent?alt=sse` (else `generateContent`,
    base64 `inlineData`), lists models and voices from `/v1beta/models` and `/v1beta/voices`,
    sends the rate as a style, honours a 429 `retryDelay`, and like `deepgram.rs` (a refused
    `speed`) drops a refused field or the stream through `Adapter::adapt`. `command.rs` kills the
    program on timeout or cancel.
- `crates/sonara-engine/src/http.rs`: the `ureq` agent shared by the external adapters and the
  Kokoro download. `src/bin/sonara-fake-tts.rs` (feature `test-util`) is the stand-in program of
  the `command` tests.
- `Engine::accepts_unlisted_voices` lets `set voice` take any id (external engines: cloud voice
  ids, cloned voices, file names of a local server).
- `crates/sonarad/src/engines.rs`: `engines.json`, one `External` per profile with Kokoro (or the
  fake engine) as its fallback, registration in the reader's and the previews' registries,
  the notice lines of `sonarad.log`, `reload`, the `E_FORBIDDEN` refusal of a `command` profile in
  `engine_add` (local-only). `engines_ext.rs`: the handlers and the voice rule: `engine_test`
  takes the request's voice, else the reader's voice when the engine is current, else the
  profile's; `engine_models` lists a saved or a draft profile's models. `engine_remove` and
  `engine_reload` live in `protocol.rs` because they may switch `engine`.
- Send mode (#235): `Engine::send_mode` and `Engine::input_limit` set `Reader::set_chunking` at
  start and on `set engine`: `Sentences`, or `Message` (one chunk per item, cut past the limit by
  `reader::pack_message`). `ReaderHandle::send_mode` tells L3, whose `Rules::set_whole_messages`
  (set by the driver before every call) makes each release of prose one entry, one item and one
  request. `Engine::lookahead` and `Reader::set_lookahead`: a cloud engine asks for two chunks
  ahead of the playing one.
- Reader streaming (#235): `Engine::streams` engines hand each piece to the worker as it arrives
  (`Done::Part`); a chunk the reader waits for plays from its first piece (`Output::play_open`,
  `append`, `finish`), and its whole audio is kept for a replay.
- `sonara-cli` `engines.rs`: `sonara engines ...`; `engines_file.rs` writes a `command` profile
  into `engines.json` locally, then sends `engine_reload`; `uninstall` deletes the `sonara:*`
  credentials unless settings are kept.

## Persisted state and logs

Everything lives in the home, `%LOCALAPPDATA%\Sonara` (`SONARA_HOME` or `--home` override it).
[PRIVACY.md](../PRIVACY.md) lists every file and what it holds; update it when adding one. The
runtime folders are `%LOCALAPPDATA%\Sonara\runtime\<version>\`.

- `config.json`: only the keys the user set (`sonarad::config`); `session_prefs.json`: label,
  voice and mute per channel; `keymap.json`: hotkey overrides (`sonara_system::keymap`);
  `engines.json`: external engine profiles, never a key; `runtime.json`: discovery; the
  crash-restore files of ducking and pausing (`sonara_system`); `stopped`: the stop sentinel.
- Logs, `logs\` (crate `sonara-log`: streams, segments, the folder budget, masking):
  - `sonarad.log`: activity lines (`support_log.rs`: start, engine readiness, `read text`; and
    `sonara_system::log`: `media pause`, `duck`, `restore`, ...) and troubleshooting lines
    (`trace_log.rs`: `in`, `agent`, `drop`, `cue`). Text and payloads only while `debug_log` is
    on. The line formats are documented at the top of those files.
  - `hook.log`: each hook's payload and what it sent (`sonara-hook`).
  - `bootstrap.log`: the runtime download and install (`bin/sonara-bootstrap.ps1`).

## How to

**Add a setting**

1. Add a `Setting` to `sonarad::config::SCHEMA` (`crates/sonarad/src/config.rs`) with its layer,
   validation and default. The default is the product's; `the_defaults_are_the_product_defaults`
   pins it.
2. Make the owning layer take it: a reader key in `sonara-reader/src/settings.rs`; an extension key
   in the extension's `KEYS` (`channels_ext`, `agent_ext`, `system_ext`) and in
   `protocol::EXTENSION_KEYS`; a host key (like `debug_log`) in `protocol.rs`. Apply the saved
   value at start (`config::apply_reader`, `agent_ext::settings_from`, the extension's enable
   path).
3. Add the control to `crates/sonarad/assets/settings.html` and run the e2e tests
   (`tests/e2e/test_sonarad_settings_e2e.py`).
4. Document the key in [protocol-v1.md](protocol-v1.md) (core "set / get" or the extension's
   Settings section) and add a conformance test (`conformance/persistence/` for the saved value).

**Add a message**

1. Dispatch it in `Server::dispatch` (`crates/sonarad/src/protocol.rs`). A core message also gets
   a capability in `CAPABILITIES` and a protocol minor (`PROTOCOL_MINOR`); an extension message
   goes in that extension's `TYPES` (which `EXTENSION_TYPES` chains), so a client without the extension
   gets `E_UNSUPPORTED`, not `E_UNKNOWN_TYPE`.
2. Handle it in the module that owns the state (`*_ext.rs`, or the layer crate through its public
   API). Take the admission lock (`self.admit()`) when it can start speech.
3. Document it in [protocol-v1.md](protocol-v1.md): changes are additive only (Versioning).
4. Add conformance tests (`conformance/<area>/`) and the call in both SDKs (`clients/ts`,
   `clients/python`).

**Add a hotkey action**

1. Add the variant to `sonara_system::keymap::Action` (`ALL`, `as_str`, `parse`; `debounced` for a
   toggle) and, only if it gets a default chord, to `DEFAULT_KEYS`. A default must be free on
   Windows 11 and must not be an AltGr character on common layouts (see the README).
2. Map it to protocol controls in `SystemExt::apply` (`crates/sonarad/src/system_ext.rs`).
3. Add a `data-action="<action>"` row to the Hotkeys page of `settings.html` and run the e2e tests.
4. Document it under Hotkeys in [protocol-v1.md](protocol-v1.md#hotkeys); test it in
   `crates/sonara-system/tests/keymap.rs` and `conformance/system/`.

**Add an external engine kind**

1. Add the variant to `Kind` in `external/profile.rs` (`ALL`, `as_str`, `display_name`,
   `default_base`, its options and rates) and in `rate.rs`.
2. Write `external/<kind>.rs` implementing `Adapter` (request, error mapping, voice and model
   lists) and return it from the `Backend` match in `external/mod.rs`.
3. Update the host and its clients: the `list["kinds"]` assertion in the engines tests of
   `crates/sonarad/src/engines.rs`, the kind
   tables of `settings.html`, the help text of `crates/sonara-cli/src/engines.rs`.
4. Document it: a section under Kinds in [protocol-v1-engines.md](protocol-v1-engines.md) and a
   row in PRIVACY.md (where the text goes, how the key is sent).
5. Test it: unit tests in the crate, a local fake of the provider in `conformance/engines/fakes.py`
   with `test_cloud_engines.py`, and an opt-in case in `tests/external_live.rs` with its env var in
   `docs/testing.md`. Never put a model id or voice name in code, defaults or docs.

**Add an earcon**

1. Add `Variant => "<kind>"` to the `earcons!` list in `crates/sonara-agent/src/earcon.rs`.
2. Render `crates/sonara-agent/sounds/<kind>.wav` with `packaging/sounds/build_earcons.py` (add
   the kind to `PICKS`) and update `sounds/SHA256SUMS` (`--check` compares them).
3. Fire it from the rules (`rules.rs`); `earcon` messages take any kind of `Earcon::ALL`, and a
   custom earcons folder may override it by file name. List the kind in
   [protocol-v1.md](protocol-v1.md#extension-agent) and test it in `conformance/agent/`.
