# Sonara runtime: a bundleable reader (spec)

Date: 2026-10-02. Issue: #168. Research: `docs/plans/2026-10-02-distribution-research.md`. Plan: `docs/plans/2026-10-02-sonara-runtime-plan.md`.

## 1. Goal

Sonara becomes a reader **component** that apps ship inside themselves. An end user downloads an app such as PrismTerminal and it can read text aloud; they never install "Sonara". A developer bundles the Sonara runtime with their app and controls it through an API: speak text, play, pause, stop, skip, go back and forward, restart, mute, volume, voice, and a live state stream to drive their own player UI.

The Claude Code plugin keeps working; it becomes one host among several.

### Decisions (user, 2026-10-02)

| # | Decision |
|---|---|
| R1 | Rewrite the core in **Rust now** (no interim frozen-Python product). |
| R2 | **GPL-free by default.** Nothing GPL ships in the runtime or reaches a host process. espeak-ng is not used. |
| R3 | **PrismTerminal** is the first app to bundle Sonara. Codex and other hosts come later. |
| R4 | **Windows first** (x64). macOS and Linux are later, enabled by the Rust core but out of scope here. |
| R5 | **Apps bundle the runtime; end users never install Sonara.** Developers control playback through the API and build their own UI. |

### Success criteria

1. A developer adds Sonara to an Electron app in under an hour: `npm i @sonara/client @sonara/runtime-win32-x64`, call `connect()`, call `speak()`, render a player from `onState()`.
2. Two apps that both bundle Sonara on one machine share one running instance: they never talk over each other, fight over ducking, or bind the same hotkeys twice.
3. Protocol v1 is documented, versioned, has a handshake with capabilities, coded errors, and a conformance suite.
4. The runtime ships no GPL code (enforced in CI by `cargo-deny`), and its default voice works with zero download (Windows OneCore). Kokoro is a pinned download on first use.
5. Reading behaviour matches today's Sonara (text cleaning golden cases, "one message, always the last", never strand ducked or paused audio).
6. The Claude Code plugin runs on the Rust runtime with feature parity, then the Python daemon is removed.

## 2. Product shape

```
 host app (PrismTerminal, a Python CLI, the Claude plugin, curl...)
   └─ client SDK (@sonara/client | sonara-client | raw protocol)   MIT, thin, no engine
        │  loopback TCP (JSON lines)  or  loopback HTTP + SSE, token auth
        ▼
 sonarad.exe  (one per user, shared by every host)                  MIT/Apache, Rust
   ├─ sonara-core      text rules, sessions, queue policy, history, state   (pure)
   ├─ sonara-engine    engine trait + registry; onecore, kokoro            (GPL-free)
   ├─ sonara-audio     output (true pause, volume, earcons mix)
   ├─ sonara-platform  ducking, media pausing, hotkeys, single instance    (Windows)
   └─ server           protocol v1, discovery, events, optional settings page
 sonara-hook.exe      tiny hook client for Claude Code (later Codex)
 models               %LOCALAPPDATA%\Sonara\models (shared, SHA-pinned, on first use)
```

**Why a shared process, not a library linked into each app:** ducking and pausing other apps, global hotkeys, one audio output and "one message, always the last" are machine-wide. Every comparable runtime that arbitrates the machine runs as a sidecar (Speech Dispatcher, Ollama). The process boundary is also the licensing firewall: hosts only link the thin MIT clients.

## 3. Instance model and discovery

- **Home:** `%LOCALAPPDATA%\Sonara` (override `SONARA_HOME`). Holds `runtime.json`, `config.json`, `state\`, `models\`, `logs\`.
- **runtime.json** (written atomically by the running instance, readable only by the user): `{pid, port, http_port, token, version, protocol: {major, minor}, started_at}`.
- **connect(runtimePath)** in every SDK:
  1. Read `runtime.json`; if the pid is alive, open a connection and send `hello`.
  2. If `hello` succeeds and `protocol.major` matches and the instance has every capability the client requires, use it.
  3. Otherwise, if `autostart` (default true), launch the bundled `runtimePath` (`sonarad.exe --home <home>`), wait up to 5 s for a fresh `runtime.json`, then `hello`.
  4. If a running instance is incompatible (older major, missing capability), the newer client sends `hello` with `takeover: true`. The instance accepts only when it is idle (nothing playing or queued) and exits after restoring audio; the client then starts its bundled runtime. If busy, the client retries takeover after the current item finishes (bounded, 30 s), then reports `E_INCOMPATIBLE`.
- **Single instance:** a per-user named mutex (`Local\Sonara-Runtime-<user-sid-hash>`), as Python 0.6.7+ does.
- **Lifetime:** the runtime exits 30 s after its last client disconnects unless a host passed `keep_alive: true` or standalone mode is on (the Claude plugin's autostart task).
- **Upgrade:** a host shipping a newer runtime takes over only when the running one is incompatible or idle-and-older; equal majors otherwise share.

## 4. Protocol v1

Full reference: `docs/protocol-v1.md` (written in milestone M5). Transport:

- **TCP JSON lines** on `127.0.0.1:<port>`. The first message must be `hello` with the token. Persistent; used by SDKs.
- **HTTP** on `127.0.0.1:<http_port>`: `POST /v1/<type>` with `Authorization: Bearer <token>` and a JSON body; `GET /v1/events` is a Server-Sent Events stream. For curl and simple hosts.
- Every request may carry an `id`; replies are `{id, ok: true, ...}` or `{id, ok: false, error: {code, message}}`. Codes: `E_AUTH`, `E_BAD_REQUEST`, `E_UNKNOWN_TYPE`, `E_UNSUPPORTED`, `E_INCOMPATIBLE`, `E_BUSY`, `E_ENGINE`, `E_NOT_FOUND`.
- **Versioning:** semver on the protocol. Minor versions only add optional fields, message types and events; unknown fields are ignored; unknown types return `E_UNKNOWN_TYPE`. Capabilities are strings in `hello` replies, e.g. `speak`, `stream`, `ask`, `controls`, `events.state`, `events.items`, `engine.kokoro`, `engine.onecore`, `hotkeys`, `settings_page`, `summaries`.

### Requests (client to runtime)

| type | fields | effect |
|---|---|---|
| `hello` | `token`, `client{name, version}`, `protocol{major,minor}`, `require[]`, `takeover?`, `keep_alive?`, `mode?: embedded\|standalone` | handshake; reply has `version`, `protocol`, `capabilities[]`, `session_defaults` |
| `speak` | `source`, `session`, `text`, `label?`, `interrupt?` | a whole message for that session (see queue policy) |
| `stream` | `source`, `session`, `turn`, `delta`, `index`, `final` | streamed message text; same rules as live prose today |
| `ask` | `source`, `session`, `kind: question\|permission\|plan`, `text`, `options?[]` | a decision needing the user; spoken with priority and its earcon |
| `turn_start` / `turn_end` | `source`, `session`, `turn` | a new turn begins (resets the session's message) / ends (`turn_done` earcon) |
| `session_open` / `session_close` | `source`, `session`, `label?`, `host_tab?`, `policy?` | register / forget a session |
| `focus` | `source`, `session` | this session is in the foreground |
| `control` | `action`, `session?` | `play`, `pause`, `toggle`, `stop`, `skip`, `restart`, `next`, `previous`, `mute_cycle`, `mute`, `unmute`, `next_session` |
| `set` / `get` | `key`, `value?` | config (rate, volume, voice, audio_mode, duck_level, policy, hotkeys...) |
| `voices` | `engine?` | list voices with engine, language, license_class, installed |
| `subscribe` | `events[]: state\|items\|log` | turn this connection into an event stream |

### Events (runtime to subscriber)

- `state`: `{seq, now_playing: {source, session, label, host_tab, item_id, text, index, count} | null, queue, paused, mute_level, volume, rate, voice, engine_status}` on change only.
- `item`: `{item_id, session, phase: started|finished|skipped|failed}`.
- `log`: diagnostics for developer tools.

### Queue policy (per session, default per source)

- `latest` (default): "one message, always the last". A new message for a session replaces that session's unread items; `restart` replays the latest message from its start.
- `queue`: every message is read in order (for hosts that want a transcript reader).
- `next` / `previous` move one spoken item (sentence or paragraph chunk) inside the current message; `restart` goes to its start. These are API controls for host players; the Claude plugin's default hotkeys keep Up (restart), Down (skip to end), M (mute), P (next session).

## 5. Engines and licensing

- **Engine trait:** `id()`, `license_class()` (`permissive` | `os`; `copyleft` is rejected at registry load in default builds), `voices()`, `warm()`, `synthesize(text, voice, rate) -> stream of PCM chunks (i16, mono, sample_rate)`, `cancel()`.
- **onecore** (Windows `Windows.Media.SpeechSynthesis` via windows-rs): zero download, always available. Reports missing voice data clearly (the failure doctor found on this PC).
- **kokoro**: Kokoro-82M v1.0 (Apache-2.0 weights), English, through ONNX Runtime. Phonemes come from misaki's Apache lexicons with **no espeak fallback**; unknown words go through a permissive fallback (letter spelling, plus a small rule set for code identifiers, paths and acronyms). The exact stack (ort + misaki-rs, or sherpa-onnx C API fed with tokens) is chosen by the M0 spike against exit criteria in the plan.
- **Models:** downloaded on first use to `%LOCALAPPDATA%\Sonara\models\<engine>\<version>\` with pinned SHA-256, resumable, with progress in `state.engine_status`. A host may pre-seed the folder. fp32 or fp16 (int8 measured slower on x86).
- **Enforcement:** `cargo-deny` licenses allowlist (MIT, Apache-2.0, BSD-2/3, ISC, Zlib, Unicode-3.0, MPL-2.0 file-level only if reviewed) and a bans list for `espeak*`, `piper-phonemize*`; `THIRD_PARTY_NOTICES.md` generated in release builds. JS clients have zero runtime dependencies.

## 6. Audio and platform (Windows)

- Output through WASAPI (cpal or rodio) on its own thread: true pause and resume, per-instance volume, earcons mixed rather than cutting speech.
- Other apps: duck by per-session volume (IAudioSessionManager2) or pause media (GlobalSystemMediaTransportControls), with today's guarantees: per-session handling, crash-restore state file, never record a ducked level as the original, restore on cancel (the #131 rules).
- Hotkeys (`global-hotkey` or RegisterHotKey): **off in embedded mode**, on in standalone mode. Hosts own their keys and call `control`.
- Settings page: served by `sonarad` in standalone mode only (the current `settings.html`, adapted to protocol v1). Embedded hosts build their own UI.

## 7. Hosts and SDKs

- **@sonara/client** (npm, TypeScript, zero dependencies, Node 18+ and Electron main process): `connect(opts)`, `speak`, `stream`, `ask`, `control`, `set/get`, `voices`, `onState`, `onItem`, `close`.
- **@sonara/runtime-win32-x64** (npm): the runtime binaries (`sonarad.exe`, `onnxruntime.dll`, assets, notices) for electron-builder `extraResources`; `runtimePath()` helper.
- **sonara-client** (PyPI, stdlib only): the same API in Python.
- **Release zip** `sonara-runtime-win-x64-<version>.zip` on GitHub Releases for any other host.
- **Claude Code adapter:** the plugin's hooks call `sonara-hook.exe` (Rust, starts in milliseconds), which maps hook events to protocol v1 (`stream`, `ask`, `turn_start/end`, `focus`, `session_open/close`). Summaries (`claude -p` / `codex exec`) move into `sonarad` as an optional feature with today's behaviour.
- **PrismTerminal:** bundles the runtime package, adds an "Audio mode" setting (off by default) and a player pill (restart, previous, play/pause, next, stop, mute, volume), and feeds text from agent tabs (session files, plus hooks when the Claude plugin is present). Its own plan lives in the PrismTerminal repo.

## 8. Behaviour carried over (must not regress)

- Text rules: `tests/fixtures/text_rules/*.json` golden cases pass in Rust byte for byte.
- "One message, always the last" for `latest` sessions; nothing silently drops the latest message (the #150 rules).
- Pause stays on when another session gets a new prompt (decision from Phase 0).
- Never strand other apps ducked or paused (#131 rules), including after a crash.
- Earcons: choice, permission, error, turn_done, nav, nav_edge, session_change, summary_failed.
- A new user's default voice works with zero download.

## 9. Testing

- Rust unit tests per crate; the golden text fixtures as a Rust integration test.
- **Conformance suite** (`conformance/`, pytest, black box over TCP and HTTP): protocol behaviour, queue policy, controls, events, errors, takeover. Runs in CI against `sonarad`; the subset that maps to the old Python daemon is used to check parity before cutover.
- Behaviour parity: the Python daemon's behavioural tests that encode product rules are ported as conformance cases.
- CI (`windows-latest`): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo deny check`, conformance suite, existing Python suite until cutover, npm and Python client tests.
- Hands-on: every user-facing milestone is installed on this PC before "merge?".

## 10. Repository layout

Monorepo (this repo):

```
Cargo.toml (workspace)        crates/sonara-core, sonara-engine, sonara-audio,
                              sonara-platform, sonarad, sonara-hook
clients/ts/                   @sonara/client
packaging/npm-runtime/        @sonara/runtime-win32-x64
clients/python/               sonara-client
conformance/                  black-box suite
src/sonara/                   Python (removed at cutover, M9)
hooks/, bin/, commands/, .claude-plugin/   Claude plugin (switches to sonara-hook in M9)
```

## 11. Out of scope

Codex and other host adapters (next plan), macOS and Linux, ACP proxy, MCP server, SAPI voice, browser extension, a GPL espeak pronunciation pack, multilingual Kokoro beyond English, code signing (until SignPath enrolment).

## 12. Risks

| Risk | Mitigation |
|---|---|
| GPL-free G2P sounds worse than today's espeak-backed Kokoro on code-heavy text | M0 spike with an A/B listening check by the user before committing; letter-spelling and identifier rules; OneCore stays available |
| `ort` is still 2.0 release candidate | pin exact version; spike also evaluates sherpa-onnx C API |
| Two runtimes during the transition | Python stays the shipped plugin until M9 parity; conformance suite is the gate |
| Shared-instance version skew between apps | `hello` capabilities, `takeover` only when idle, `E_INCOMPATIBLE` surfaced to the host |
| Rewrite scope creep | port behaviour, not structure; no new features before M9 except the protocol v1 surface |
