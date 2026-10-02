# Sonara runtime: a layered, bundleable reader (spec)

Date: 2026-10-02, revised 2026-10-02 (layered design, licensing rule R6). Issue: #168. Research: `docs/plans/2026-10-02-distribution-research.md`. Plan: `docs/plans/2026-10-02-sonara-runtime-plan.md`.

## 1. Goal

Sonara becomes a **simple reader** that apps ship inside themselves, with everything else as optional layers on top. An end user downloads an app such as PrismTerminal and it can read text aloud; they never install "Sonara". A developer bundles Sonara and drives it through a small API: speak text, play, pause, stop, skip, go back and forward, restart, volume, speed, voice, and a live state stream for their own player. Agent features (sessions, questions, summaries, hotkeys, ducking) are opt-in layers, never part of the core API.

The Claude Code plugin keeps working; it becomes one product built from the layers.

### Decisions (user, 2026-10-02)

| # | Decision |
|---|---|
| R1 | Rewrite the core in **Rust now**. |
| R2 | **GPL-free by default.** Nothing GPL ships. espeak-ng is not used. |
| R3 | **PrismTerminal** is the first app to bundle Sonara. Codex and other hosts come later. |
| R4 | **Windows first** (x64). macOS and Linux later. |
| R5 | **Apps bundle Sonara; end users never install it.** Developers control it through the API and build their own UI. |
| R6 | **Bundlers are unrestricted.** Anyone bundling Sonara (including the maintainer) may sell their product, keep it closed-source, choose its licence and code-sign it. The only obligation is shipping Sonara's licence notices (`THIRD_PARTY_NOTICES.md`). Every shipped component must be under a permissive licence; CI enforces it. |
| R7 | **Layered design.** A minimal reader core; channels, agent features, system extras, adapters and UI kits are separate optional layers built only on public APIs. |

### Success criteria

1. A developer can read text aloud from an Electron app in under an hour using only the core API: `connect()`, `speak(text)`, `onState()`.
2. A developer can build a full player (play/pause, back/next, restart, mute, volume, speed, progress) from the core API alone, without any agent concept.
3. Two apps that both bundle Sonara share one running instance and never talk over each other.
4. Protocol v1 has a small, documented core plus versioned extensions, a handshake with capabilities, coded errors and a conformance suite.
5. No GPL or other copyleft code ships (CI `cargo-deny`); every shipped licence permits commercial, closed-source, signed distribution. The default voice works with zero download (Windows OneCore); Kokoro is a pinned download on first use.
6. Reading behaviour matches today's Sonara (golden text cases; the Claude product keeps "one message, always the last" and never strands ducked audio).
7. The Claude Code plugin runs on the new runtime with feature parity; then the Python daemon is removed.

## 2. Layers

Each layer depends only on the layers below it, through their public API. A host enables only what it needs.

| Layer | Crate / package | Contains | Not in it |
|---|---|---|---|
| **L1 Reader core** | `sonara-core` (pure), `sonara-engine`, `sonara-audio`, facade `sonara-reader` | text rules and sentence chunking; one queue of **items** (an item = one text, split into chunks); controls `play`, `pause`, `toggle`, `stop`, `skip` (next item), `previous`/`next` (chunk), `restart` (item start), `mute`/`unmute`, volume, rate, voice; queue mode `append` or `replace`; state and item events; engine trait with OneCore and Kokoro; audio output with true pause | sessions, sources, turns, questions, earcons, summaries, ducking, hotkeys |
| **L2 Channels** | `sonara-channels` | several named sources (tabs, chats), each with its own queue and policy `latest` or `queue`; which channel is in front; `next_channel`; announcing a channel switch | anything agent-specific |
| **L3 Agent** | `sonara-agent` | streaming text (deltas), turns (`turn_start`/`turn_end`), decisions (`ask`: question, permission, plan) spoken with priority, earcons, three-level mute (earcons muted too), optional summaries (`claude -p` / `codex exec`) | host-specific hook formats |
| **L4 System** | `sonara-system` (Windows) | duck or pause other apps' audio with crash restore; global hotkeys; the settings page | product logic |
| **L5 Adapters** | `sonara-hook` (Claude Code; Codex later) | translate a host's hook events into L2/L3 messages | reading logic |
| **L6 UI kits** | `@sonara/player` | a headless player controller and a React player component built on the L1 API | runtime logic |

`sonarad.exe` is a **host process**, not a layer: it runs the reader and whichever layers are enabled, and serves the protocol.

### Two ways to use L1

- **In-process (Rust):** `sonara-reader` is a library: `Reader::new(config)`, `reader.speak(text)`, `reader.control(..)`, `reader.subscribe()`. One app, its own audio, no other process. Bindings for other languages (C ABI, Node addon) are a later option, not v1.
- **Shared process (any language):** `sonarad.exe` hosts the reader and layers; clients speak protocol v1. This is the default for non-Rust hosts and whenever several apps or agents must share one audio output. The process boundary also keeps the engine out of host processes.

## 3. Instance model and discovery (shared process)

- **Home:** `%LOCALAPPDATA%\Sonara` (override `SONARA_HOME`): `runtime.json`, `config.json`, `state\`, `models\`, `logs\`.
- **runtime.json** (atomic, user-only): `{pid, port, http_port, token, version, protocol: {major, minor}, capabilities, started_at}`.
- **connect(runtimePath)** in every SDK:
  1. Read `runtime.json`; if the pid is alive, connect and send `hello`.
  2. Use it if `protocol.major` matches and it offers every capability the client `require`s.
  3. Otherwise, if `autostart` (default true), launch the bundled `sonarad.exe --home <home>`, wait up to 5 s for a fresh `runtime.json`, then `hello`.
  4. If the running instance is incompatible, the client sends `hello` with `takeover: true`; the instance accepts only when idle (nothing playing or queued), restores audio and exits; the client starts its bundled runtime. If busy, retry after the current item (bounded, 30 s), then `E_INCOMPATIBLE`.
- **Single instance:** per-user mutex `Local\Sonara-Runtime-<user-sid-hash>`.
- **Lifetime:** exits 30 s after the last client disconnects unless a client set `keep_alive` or standalone mode is on.
- **Layers per client:** a client asks for extensions in `hello` (`extensions: ["channels", "agent", "system"]`); the instance enables an extension when any connected client asks for it. Messages of an extension no client enabled return `E_UNSUPPORTED`.

## 4. Protocol v1

Full reference: `docs/protocol-v1.md` (written with the core server milestone). Transport: TCP JSON lines on `127.0.0.1:<port>` (first message `hello` with the token) and HTTP on `127.0.0.1:<http_port>` (`POST /v1/<type>`, `Authorization: Bearer <token>`; `GET /v1/events` as Server-Sent Events). Never bind non-loopback.

Every request may carry `id`; replies are `{id, ok: true, ...}` or `{id, ok: false, error: {code, message}}`. Codes: `E_AUTH`, `E_BAD_REQUEST`, `E_UNKNOWN_TYPE`, `E_UNSUPPORTED`, `E_INCOMPATIBLE`, `E_BUSY`, `E_ENGINE`, `E_NOT_FOUND`. Versioning: semver; minors only add optional fields, types and events; unknown fields are ignored.

### 4.1 Core (always available)

| type | fields | effect |
|---|---|---|
| `hello` | `token`, `client{name, version}`, `protocol{major,minor}`, `require[]`, `extensions[]`, `takeover?`, `keep_alive?` | handshake; reply `{version, protocol, capabilities[], extensions[]}` |
| `speak` | `text`, `mode?: append\|replace` (default `append`), `interrupt?`, `label?` | add an item; reply `{item_id}`. `replace` drops unread items first; `interrupt` also cuts the current one |
| `control` | `action: play\|pause\|toggle\|stop\|skip\|previous\|next\|restart\|mute\|unmute` | playback control. `previous`/`next` move one chunk within the current item; `skip` ends the current item; `restart` goes to its first chunk |
| `set` / `get` | `key` (`volume`, `rate`, `voice`, `engine`), `value?` | settings |
| `voices` | `engine?` | voices with engine, language, licence class, installed |
| `subscribe` | `events[]: state\|items\|log` | event stream on this connection |

Events: `state` `{seq, now_playing: {item_id, label, text, chunk, chunks} | null, queued, paused, muted, volume, rate, voice, engine_status}` on change only; `item` `{item_id, phase: started|finished|skipped|failed}`; `log`.

### 4.2 Extension `channels`

`speak` and `control` gain `channel?`. New types: `channel_open {channel, label?, host_tab?, policy?: latest|queue}`, `channel_close`, `focus {channel}`; `control` gains `next_channel`. `state.now_playing` gains `channel`, `host_tab`. Policy `latest`: a new item for a channel replaces that channel's unread items ("one message, always the last").

### 4.3 Extension `agent` (requires `channels`)

`stream {channel, turn, delta, index, final}`, `turn_start {channel, turn}` (resets the channel's message; drops late text from the previous turn), `turn_end {channel, turn}` (turn_done earcon), `ask {channel, kind: question|permission|plan, text, options?}`, `earcon {kind}`, `set mute_level 0|1|2`, `set summaries {...}`. Messages may carry `t` (sender start time) so late text from a previous turn is dropped (the #174 rule).

### 4.4 Extension `system`

`set audio_mode duck|pause|off`, `set duck_level`, `set hotkeys {...}`, `get settings_url`. Hotkeys call the same controls as `control`. Off unless a client enables it (the Claude product does).

## 5. Engines and licensing

- **Engine trait** (L1): `id()`, `license_class()` (`permissive` | `os`; anything else is refused at load), `voices()`, `warm()`, `synthesize(text, voice, rate) -> stream of PCM chunks`, `cancel()`.
- **onecore** (Windows `Windows.Media.SpeechSynthesis`): zero download, always available; reports missing voice data clearly.
- **kokoro**: Kokoro-82M v1.0 (Apache-2.0 weights), English, via ONNX Runtime (MIT, Microsoft's official CPU build next to the exe). Phonemes from misaki's lexicons (MIT/Apache) with no espeak fallback; unknown words through a permissive fallback (word splitting, letter-to-sound rules, spelling) and a small custom lexicon (Sonara, Kokoro, onnx...). Stack chosen in M0: `ort` + a vendored `misaki-rs` (pending the user's listening check).
- **Models:** downloaded on first use to `%LOCALAPPDATA%\Sonara\models\<engine>\<version>\`, SHA-256 pinned, resumable, progress in `state.engine_status`; a host may pre-seed the folder.
- **R6 enforcement:** `cargo-deny` licence allowlist (MIT, Apache-2.0, BSD-2/3, ISC, Zlib, Unicode-3.0) and bans (`espeak*`, `piper-phonemize*`); JS and Python clients have zero runtime dependencies; `THIRD_PARTY_NOTICES.md` (code and model/data licences, including the Kokoro weights and the misaki lexicon attribution) generated for every release; the VC++ runtime DLLs are Microsoft-redistributable and listed. Data provenance (lexicon entries corrected with espeak's output) is recorded in the notices and reviewed before the first commercial bundle.

## 6. Audio and system (Windows)

- L1 output through WASAPI (`rodio`/`cpal`) on its own thread: true pause and resume (verified in M0), volume, earcons mixed rather than cutting speech (earcons are an L3 feature using an L1 output hook).
- L4: duck by per-session volume or pause media (GSMTC) with today's guarantees (per-session handling, crash-restore file, never record a ducked level as the original, restore on cancel and on client disconnect).
- L4 hotkeys and settings page only when a client enables `system`.

## 7. Hosts and SDKs

- **@sonara/client** (npm, TypeScript, zero deps, Node 18+/Electron main): core API first (`connect`, `speak`, `control`, `set/get`, `voices`, `onState`, `onItem`, `close`); extensions as namespaces (`client.channels.*`, `client.agent.*`, `client.system.*`).
- **@sonara/runtime-win32-x64** (npm): runtime binaries for electron-builder `extraResources`, `runtimePath()`.
- **@sonara/player** (L6): headless `PlayerController` (state to view-model, buttons to controls) plus a React `<SonaraPlayer/>`.
- **sonara-client** (PyPI, stdlib only): the core API and extension namespaces in Python.
- **Release zip** `sonara-runtime-win-x64-<version>.zip` for any other host.
- **Claude Code product:** plugin hooks call `sonara-hook.exe` (L5), which uses `channels` + `agent`; the plugin enables `system` (ducking, hotkeys Ctrl+Alt+Up/Down/M/P, settings page).
- **PrismTerminal:** bundles the runtime and `@sonara/player`; "Audio mode" setting (off by default); feeds text per tab through `channels` (and `agent` when the Claude plugin's hooks are present). Its own plan lives in the PrismTerminal repo.

## 8. Behaviour carried over (must not regress)

- L1: golden text cases byte for byte; restart, previous/next, pause/resume never lose or duplicate text; rapid repeated controls behave (the #128 lesson).
- L2: `latest` policy and nothing silently drops the latest message (the #150 rules); a channel switch is announced.
- L3: pause stays on when another channel gets a new turn; late text from a previous turn is dropped (#174); earcons choice, permission, error, turn_done, nav, nav_edge, session_change, summary_failed.
- L4: never strand other apps ducked or paused (#131), including after a crash.
- The default voice works with zero download.

## 9. Testing

- Rust unit tests per crate; golden text fixtures as an integration test (done in M1).
- Each layer has its own tests against the public API of the layer below (no reaching into internals).
- **Conformance suite** (`conformance/`, pytest, black box): one module per protocol part (core, channels, agent, system); a host passes "core" without implementing any extension.
- Parity: the Python daemon's behavioural tests that encode product rules are ported as `channels`/`agent` conformance cases.
- CI (`windows-latest`, `cargo-deny` on Linux): fmt, clippy, test, deny, conformance, Python suite until cutover, client tests.
- Hands-on: every user-facing milestone is installed on this PC before "merge?".

## 10. Repository layout

```
Cargo.toml (workspace)
crates/sonara-core        L1 pure: text rules, assembler, reader state machine
crates/sonara-engine      L1 engine trait, onecore, kokoro (features)
crates/sonara-audio       L1 output
crates/sonara-reader      L1 facade (in-process library)
crates/sonara-channels    L2
crates/sonara-agent       L3
crates/sonara-system      L4 (Windows)
crates/sonarad            host process: protocol server, discovery, settings page assets
crates/sonara-hook        L5 Claude Code adapter
clients/ts                @sonara/client
clients/player            @sonara/player (L6)
packaging/npm-runtime     @sonara/runtime-win32-x64
clients/python            sonara-client
conformance/              black-box suite (core, channels, agent, system)
src/sonara/               Python (removed at cutover)
hooks/, bin/, commands/, .claude-plugin/   Claude plugin (switches to sonara-hook at cutover)
```

## 11. Out of scope

Codex and other adapters (next plan), macOS and Linux, C ABI / Node addon for in-process L1, ACP proxy, MCP server, SAPI voice, browser extension, a GPL espeak pack (excluded by R6), multilingual Kokoro, code signing of Sonara's own release (until SignPath enrolment; bundlers sign their own products freely).

## 12. Risks

| Risk | Mitigation |
|---|---|
| GPL-free pronunciation is worse on code-heavy text | M0 blind listening check (7 of 10); custom lexicon and identifier rules; OneCore available |
| Layer boundaries erode (agent ideas creep into L1) | L1 crates must not depend on L2+ crates (enforced by the workspace dependency graph and a test); conformance "core" runs with no extension enabled |
| `ort` is a 2.0 release candidate | pin exact version; official ORT build |
| Two runtimes during the transition | Python stays the shipped plugin until cutover; conformance suite is the gate |
| Shared-instance version skew | `hello` capabilities, idle-only takeover, `E_INCOMPATIBLE` |
