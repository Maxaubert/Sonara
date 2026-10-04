# Sonara protocol v1

The contract between `sonarad.exe` (the Sonara runtime) and its clients: apps that bundle Sonara, SDKs (`@sonara/client`, `sonara-client`) and anything else on the same PC. This document covers the **core** (protocol 1.2), which every runtime offers, [external engines](#external-engines) (capability `engines`), and the [`channels`](#extension-channels), [`agent`](#extension-agent) and [`system`](#extension-system) extensions.

Source of truth in code: `crates/sonarad` (server), `crates/sonara-reader` (the reader behind it), `crates/sonara-channels` (the `channels` extension), `crates/sonara-agent` (`agent`), `crates/sonara-system` (`system`). Black-box tests: `conformance/` (`python -m pytest conformance -q` after `cargo build -p sonarad`). Spec: `docs/plans/2026-10-02-sonara-runtime-spec.md` sections 3 and 4.

The Python daemon of the Claude Code plugin still speaks the older protocol in `docs/protocol.md` until the cutover (M11).

## Discovery

**Home folder:** `%LOCALAPPDATA%\Sonara`, or `SONARA_HOME` if set, or `sonarad --home <dir>` (the flag wins).

**`runtime.json`** in the home describes the running instance. It is written atomically (a temp file, then a rename) with an ACL that lets only the current user in, and removed when the runtime exits cleanly.

```json
{
  "pid": 12345,
  "port": 50311,
  "http_port": 50312,
  "token": "64 hex characters",
  "version": "0.9.7",
  "protocol": {"major": 1, "minor": 2},
  "capabilities": ["core", "speak", "control", "set", "get", "voices", "subscribe", "events.state", "events.items", "events.log", "engine_status", "engines"],
  "extensions": ["channels", "agent", "system"],
  "started_at": "2026-10-02T10:40:45Z"
}
```

**Connecting** (what every SDK's `connect()` does):

1. Read `runtime.json`. If the file exists and its `pid` is alive, connect to `port` and send `hello`.
2. Use the instance if `protocol.major` is 1 and its `capabilities` (and `extensions`) cover everything the client requires.
3. Otherwise start the bundled runtime: `sonarad.exe --home <home>` (no console window: `CREATE_NO_WINDOW`), wait up to 5 s for a `runtime.json` whose `pid` is the new process, then `hello`.
4. If the running instance is incompatible (another major, a missing capability), send `hello` with `takeover: true`. See [Takeover](#takeover).

A stale `runtime.json` (the pid is gone after a crash) is overwritten by the next runtime.

**One instance** per user: the runtime holds the named mutex `Local\Sonara-Runtime-<hash>`, where `<hash>` is the FNV-1a 64-bit hash (16 hex digits) of the user's SID string. For a home other than the default `%LOCALAPPDATA%\Sonara`, the hash covers the SID, a newline and the home's canonical path in lower case, so separate homes (tests, a portable bundle) are separate instances. A second `sonarad` for the same user and home prints `another instance is already running ...` and exits with code 3.

## Transport

Both transports bind `127.0.0.1` on ephemeral ports and never another address. Every request is a JSON object with a `type`; requests are applied in the order they arrive on a connection.

### TCP JSON lines (`port`)

- One UTF-8 JSON object per line (`\n`), both ways; at most 1 MiB per line.
- The **first message must be `hello` with the token**. Anything else (another type, a wrong token, invalid JSON) is answered with `E_AUTH` and the connection closes. So is a connection that sends no successful `hello` within 5 s. A `hello` that fails for another reason (`E_UNSUPPORTED`, `E_INCOMPATIBLE`, `E_BUSY`) leaves the connection open and unauthenticated, so the client may send `hello` again.
- Replies and events share the connection: a reply has `ok`, an event has `event`. Replies come in request order.
- Invalid JSON after `hello` is `E_BAD_REQUEST`; the connection stays open.

### HTTP (`http_port`)

- `POST /v1/<type>` with header `Authorization: Bearer <token>`. The body is the request without `type` (the path gives it), a JSON object; an empty body is `{}`. At most 1 MiB.
- No `hello` is needed (the bearer token authenticates each request), but `POST /v1/hello` works, including `keep_alive` and `takeover`.
- The reply body is the same JSON as on TCP. Status: 200 for `ok: true`; for errors `E_AUTH` 401, `E_UNKNOWN_TYPE` and `E_NOT_FOUND` 404, `E_BUSY` and `E_INCOMPATIBLE` 409, `E_ENGINE` 500, anything else 400.
- `GET /v1/events?events=state,items,log` (default: all three) is a Server-Sent Events stream. Each event is `event: <name>` plus `data: <the same JSON as on TCP>`; a `: ping` comment comes every 15 s. `subscribe` over POST is `E_BAD_REQUEST`.

## Requests and replies

Every request may carry `id` (any JSON value); the reply echoes it. Replies are `{id?, ok: true, ...}` or `{id?, ok: false, error: {code, message}}`. **Unknown fields are ignored** in every request, and clients must ignore fields they do not know in replies and events.

### `hello`

| field | |
|---|---|
| `token` | from `runtime.json` (TCP: required) |
| `client` | `{name, version}`, informational |
| `protocol` | `{major, minor}` the client speaks; a major other than 1 is `E_INCOMPATIBLE` (omitted: 1.0) |
| `require[]` | capabilities or extensions the client cannot work without; any missing is `E_UNSUPPORTED`. A required extension is also enabled |
| `extensions[]` | extensions the client would like enabled; those this runtime lacks are listed in `unavailable`, not an error |
| `takeover?` | `true`: ask the runtime to exit for a newer one (see [Takeover](#takeover)) |
| `keep_alive?` | `true`: the runtime keeps running after the last client left (until a takeover or the process is ended) |

```json
> {"type": "hello", "id": 1, "token": "...", "client": {"name": "prism", "version": "2.1"}, "protocol": {"major": 1, "minor": 0}, "require": ["core"], "extensions": ["channels"]}
< {"id": 1, "ok": true, "version": "0.10.0", "protocol": {"major": 1, "minor": 1}, "capabilities": ["core", "speak", ...], "extensions": ["channels"], "unavailable": []}
```

The reply's `extensions` lists the extensions enabled on this runtime now. An extension is enabled for the whole runtime as soon as any client asks for it (in `extensions` or `require`) and stays enabled until the runtime exits; until then its messages, actions and keys are `E_UNSUPPORTED`. `runtime.json` lists in `extensions` the ones this runtime offers.

**Capabilities** of protocol 1.0: `core`, `speak`, `control`, `set`, `get`, `voices`, `subscribe`, `events.state`, `events.items`, `events.log`. Protocol 1.1 adds `engine_status` (readiness and model download progress in `state.engine_status`). A later minor adds capability strings for what it adds, so a client can `require` them.

### `speak`

Add one text as one **item** (its sentences are its chunks).

| field | |
|---|---|
| `text` | required string. Text with nothing speakable becomes an item that is reported `skipped` at once |
| `mode?` | `append` (default): after everything queued. `replace`: drop the unread items first (each `skipped`), never the current one |
| `interrupt?` | `true`: also cut the current item (`skipped`) and start this one now |
| `label?` | shown in `state.now_playing.label` |

```json
> {"type": "speak", "text": "Build finished. Two warnings.", "label": "build"}
< {"ok": true, "item_id": 7}
```

Item ids start at 1 and never repeat within one runtime.

### `control`

`{"type": "control", "action": "<action>"}`, reply `{ok: true}` once carried out.

| action | effect |
|---|---|
| `play` / `pause` / `toggle` | resume / hold / flip the current item. No-ops when nothing is playing (never paused with nothing to read). A pause holds across navigation and new items |
| `stop` | end the current item and clear the queue (all `skipped`) |
| `skip` | end the current item (`skipped`) and start the next |
| `previous` / `next` | one chunk back / forward within the current item; `previous` on the first chunk restarts it, `next` on the last is `skip` |
| `restart` | back to the current item's first chunk; when idle, replay the last item that ended (not after `stop`) as a new item |
| `mute` / `unmute` | silence the output and back; playback keeps moving, and the mute survives `stop` and new items |

### `set` / `get`

`{"type": "set", "key": "<key>", "value": <value>}` and `{"type": "get", "key": "<key>"}`; both reply `{ok: true, key, value}` with the value now in force.

| key | value |
|---|---|
| `volume` | integer 0..=100 (percent) |
| `rate` | integer 100..=400 (words per minute) |
| `voice` | a voice `id` or `name` of the current engine; `null` for the engine default. `get` returns the id or `null` |
| `engine` | an engine id: `kokoro` or `onecore` (`fake` in test runs), or the id of an [external engine](#external-engines) the user added. Switching resets a voice the new engine lacks |
| `debug_log` | `true` or `false` (default `true`, runtime 0.13.2, #219): the troubleshooting log records text (what was read, the messages received, the hooks' raw input); `false` keeps every text out of `logs\` (see [Saved settings](#saved-settings)). A host key: it needs no extension |

A rate, voice or engine change applies to chunks synthesized from then on. Out-of-range or wrongly typed values are `E_BAD_REQUEST`; an unknown voice or engine is `E_NOT_FOUND`; an unknown key is `E_BAD_REQUEST` (an extension's key, such as `audio_mode`, is `E_UNSUPPORTED`).

### `voices`

`{"type": "voices", "engine?": "<id>", "refresh?": false}` replies `{ok: true, voices: [...]}` for one engine or all:

```json
{"id": "...", "name": "Microsoft Zira", "language": "en-US", "engine": "onecore", "license_class": "os", "installed": true}
```

`license_class` is `permissive`, `os` or `external` (protocol 1.2, the voices of an [external engine](#external-engines)). `installed: false` means listed but not yet able to speak (voice data missing, a model still to download). An unknown engine is `E_NOT_FOUND`.

For an external engine (`engine` naming one), the runtime asks its provider for the list when its copy is older than 10 minutes or `refresh` is `true` (the request waits up to 10 s). A failed fetch still answers `ok` with the voices known (at least the engine's own voice) and an additive `error: {reason, message}`. Without `engine`, external engines contribute the lists they already have, never a network request. An external engine also speaks voice ids it does not list (a cloned voice, a provider's voice id), so `set voice` accepts any id while one is current.

**Engines.** `onecore` is Windows' own speech: zero download, licence class `os`. `kokoro` is Kokoro-82M v1.0 (Apache-2.0 weights) on Microsoft's ONNX Runtime with GPL-free phonemes, licence class `permissive`, 28 English voices (`af_heart`, the default, `af_sarah`, `bm_george`, ...; ids also accept the `kokoro:` prefix and display names such as `Heart (Kokoro)`). Its model (about 354 MB) is downloaded on first use into `<home>\models\kokoro\v1.0\` from pinned URLs with pinned SHA-256 values, resumed after an interruption (a download in progress is `<file>.part`), and checked before use (`verified.json` there remembers checked files by size and time); a host may pre-seed that folder with the two files (`kokoro-v1.0.onnx`, `voices-v1.0.bin`). Until Kokoro is ready (downloading, a failed download waiting to retry, no ONNX Runtime) it speaks with `onecore` at once, and `state.engine_status` says so. Where `onecore` cannot speak (its warm-up fails or it lists no voices), there is no fallback: `engine_status` names none, and each item waits for Kokoro while the model downloads or loads (up to 5 minutes), so no speech is dropped. The runtime does not idle out while the model downloads or loads. A failed download is retried after 30 s, then after twice as long each time up to 30 minutes, never on every sentence. The rate maps to Kokoro's speed as `rate / 200`, from 0.5 to 2.0.

### `subscribe` (TCP)

`{"type": "subscribe", "events": ["state", "items", "log"]}` (omitted: all three) replies `{ok: true, events: [...]}` and from then on sends those events on this connection. Subscribing again replaces the set (`[]` stops events). An unknown stream name is `E_UNSUPPORTED`; an extension's stream (`earcons` of `agent`, `cues` of `system`) is asked for by name and is `E_UNSUPPORTED` while the extension is off. When `state` is included, the first event is the current state.

A client that does not read its events never slows the reader: past 256 unread events, events are dropped for that client. Each `state` event is a full snapshot, so the next one brings a player up to date.

## Events

```json
{"event": "state", "seq": 12, "now_playing": {"item_id": 7, "label": "build", "text": "Build finished.", "chunk": 0, "chunks": 2}, "queued": 0, "paused": false, "muted": false, "volume": 100, "rate": 200, "voice": null, "engine_status": {"engine": "kokoro", "ready": true, "status": "ready"}}
{"event": "item", "item_id": 7, "phase": "started"}
{"event": "log", "message": "synthesis failed: ..."}
```

- `state` (stream `state`): sent on change only, `seq` strictly increasing. `now_playing` is `null` when idle; its `text` is the chunk being read. `queued` counts items after the current one. `engine_status` (below) says whether the current engine speaks with its own voice yet.
- `item` (stream `items`): `phase` is `started`, `finished`, `skipped` or `failed`. An item ends `failed` only when none of its chunks could be played; a failed chunk is skipped and logged.
- `log` (stream `log`): a line worth showing in a log, such as a failed synthesis or an engine that is not ready. A change of the engine's readiness is logged too (`engine 'kokoro' is downloading its model; speaking with onecore meanwhile`, `engine 'kokoro' is ready`), not each bit of download progress.

**`engine_status`** (protocol 1.1; a 1.0 runtime sends only `engine`):

| field | meaning |
|---|---|
| `engine` | the current engine id |
| `ready` | `true` when it speaks with its own voice |
| `status` | `ready`, `loading` (its model, a few seconds), `downloading`, `waiting` (the last download or load failed; it retries later) or `unavailable` (it cannot run in this install, for Kokoro: no `onnxruntime.dll`) |
| `progress` | `{done, total}` bytes, while `downloading` |
| `fallback` | the engine speaking meanwhile (`onecore`), while not ready |
| `message` | why it is not ready, after a failure |
| `reason` | protocol 1.2, external engines: why it is not speaking itself, one of `no_key`, `auth`, `quota`, `rate_limited`, `network`, `timeout`, `server`, `bad_voice`, `bad_config`, `format` (see [External engines](#external-engines)) |

```json
"engine_status": {"engine": "kokoro", "ready": false, "status": "downloading", "progress": {"done": 104873984, "total": 353746785}, "fallback": "onecore"}
```

A change of `engine_status` alone (download progress, about four times a second at most) is a new `state` event with a new `seq`. The runtime counts these changes once, so every client sees the same `seq` for the same state, however long it has been subscribed.

## Errors

| code | when |
|---|---|
| `E_AUTH` | missing or wrong token; on TCP a first message other than `hello`, or no `hello` within 5 s (the connection closes) |
| `E_BAD_REQUEST` | not a JSON object, missing `type`, a missing or wrongly typed field, an out-of-range value, an unknown action or key |
| `E_UNKNOWN_TYPE` | a `type` this protocol does not define |
| `E_UNSUPPORTED` | an unmet `require`, an unknown event stream, or a message, action or key of an extension that is not enabled |
| `E_INCOMPATIBLE` | `hello` with another protocol major |
| `E_BUSY` | `hello` with `takeover: true` while something is playing or queued; `speak` or `control` after a takeover was accepted or the idle exit was decided; any request while the runtime is exiting |
| `E_ENGINE` | the engine or the reader failed; a Credential Manager failure; a failed `engine_test` |
| `E_NOT_FOUND` | an unknown voice, engine or external engine profile |

An error of an external engine (`engine_test`) carries an additive `reason` (protocol 1.2), the same values as `engine_status.reason`: `{"ok": false, "error": {"code": "E_ENGINE", "message": "OpenAI refused the key (401): Incorrect API key provided", "reason": "auth"}}`.

## Lifetime

A client is a TCP connection that completed `hello`, or an open SSE stream; a plain HTTP request only counts as activity. The runtime exits 30 s (`--idle-exit <seconds>`) after the last client left and nothing is being read (a paused item does not count), unless a client sent `keep_alive: true` or it runs with `--standalone`. The countdown starts when `runtime.json` is written, so a slow start never uses it up before the first client can connect. The exit is decided atomically with the requests that start speech: a `speak` or `control` either comes first (and keeps the runtime while it is read) or is answered `E_BUSY`, never accepted and then lost; any request that reaches the runtime while it exits is `E_BUSY` (#194). Ctrl+C and a [`shutdown`](#shutdown) (extension `system`) end it cleanly. On every clean exit it stops speech and removes `runtime.json`.

## Takeover

When a client finds an instance it cannot use (another protocol major, a missing capability):

1. It sends `hello` with the token and `takeover: true`.
2. If nothing is playing or queued (a paused item counts as busy), the runtime replies `{ok: true, takeover: true, ...}` and from then on answers `speak` and `control` from any client with `E_BUSY`, so nothing is accepted and then dropped. It closes the connection, stops audio, releases the single-instance lock, removes `runtime.json` and exits with code 0. The client waits for the process to end (or, at the least, for `runtime.json` to go), then starts its bundled runtime.
3. Otherwise the reply is `E_BUSY`; the client retries after the current item (bounded, 30 s in total), then gives up with `E_INCOMPATIBLE`.

## Versioning

Semantic versioning on `protocol: {major, minor}`. A minor only adds optional fields, message types, capabilities and events; it never changes the meaning of what exists. 1.1 (runtime 0.10.0) added the readiness fields of `engine_status` and the `engine_status` capability. 1.2 (runtime 0.15.0, #224) added [external engines](#external-engines): the capability `engines` and its five core messages, `engine_status.reason`, `error.reason`, `voices.refresh` and the licence class `external`. Runtime 0.13.0 (#214) narrowed `verbosity` to `everything` and `skip_code` (old values are accepted as aliases: `all` is `everything`, `medium` and `quiet` are `skip_code`, so `get` returns the new names) and added `engine_status` to `runtime`; no shipped client depended on the old verbosity values, so the protocol minor was not bumped for it. Clients ignore unknown fields and event types. A new major is a new protocol: a client that needs it takes over an idle older runtime.

## Saved settings

Every setting a client changes with `set` is saved in the home and applies again when the next runtime starts (#201): the core keys before the runtime accepts its first client (so before the first speech), an extension's keys when a client enables that extension. Hotkeys that change a setting (the mute cycle, faster, slower) save it too.

**Defaults** (#202). A setting nobody set has the runtime's default, the Claude plugin's product settings: `voice` `af_sarah` (with an engine that has it; otherwise the engine's default voice), `rate` 250, `volume` 100, `channel_announce` `"on"`, `mute_level` 0, `verbosity` `"skip_code"`, `read_mode` `"done"` (#222), `flush_scope` `"session"` (#228), `minqueue` 5, `background_policy` `"all"`, summaries off, `audio_mode` `"pause"`, `duck_level` 30, `debug_log` true. The tables below give the same defaults. `sonarad` applies them to the layers itself; the library crates keep their own (rate 200, `"everything"`, `"queue"` with 1, `"earcon_only"`, `"off"`).

| file in the home | holds |
|---|---|
| `config.json` | only the keys a client set (even to the default), never the defaults: `engine`, `voice`, `rate`, `volume`, `channel_announce`, `mute_level`, `verbosity`, `read_mode`, `flush_scope`, `minqueue`, `background_policy`, `summaries` (only the fields that were set, plus `prompts`), `audio_mode`, `duck_level`, `debug_log`. A `_migrated` key records the migration below |
| `session_prefs.json` | per channel: `label`, `voice`, `muted` (see `channel_prefs`), the 200 most recently changed |
| `engines.json` | the [external engines](#external-engines) added with `engine_add`: `{"format": 1, "engines": [profile, ...]}`, never a key. A file that is not JSON is copied to `engines.json.bad` (and logged) and counts as empty until the next change; an entry that fails validation or names a kind this runtime lacks is kept, listed and not used. Read once at start, before the first speech, so a saved `engine` naming one applies at once |
| `keymap.json` | the hotkey overrides (see [Hotkeys](#hotkeys)) |
| `earcons\` | the user's own earcons: `<kind>.wav` replaces that kind's bundled clip (see [Custom earcons](#custom-earcons)); created empty at start, only read |
| `logs\` | every log of the home, under one budget (#219, crate `sonara-log`): each stream (`sonarad.log`, `hook.log`) rotates at about 1 MB into `<stream>.1.log`, `<stream>.2.log`, ... (1 the newest), and everything in the folder together (also files other writers leave there, such as `bootstrap.log`) stays at or under 10 MB: a line that would pass it first deletes the oldest files (by last write), and as a last resort empties the stream's own file (when a viewer holding an older segment stopped its rotation). Writers in several processes (the runtime's threads, each hook call) take an OS lock on `logs\.lock` per line, so no line is torn, lost or duplicated across a rotation; a writer (the runtime or a hook) waits 50 ms for it at most and otherwise skips its line. A line over 256 KB is clipped |
| `logs\sonarad.log` | each line starts with a UTC time with milliseconds (`2026-10-03T10:59:14.123Z`). One line per start (`sonarad <version> started (pid <pid>): engine <id> <status>; home <dir>`), the engine's readiness changes (model downloaded, loaded or failed; not the progress), external engines (`engine add id=<id> kind=<kind> host=<host>[ key=set]`, `engine remove id=<id>`, `engine key id=<id> set\|cleared`, never the key; `engine <id> fallback reason=<reason>[ status=<http>] -> <fallback>: <message>` when the built-in voice read instead, at most one per engine and reason per minute, and `engine <id> recovered`, never the text), the migration, saved values that could not be applied, custom earcons used or refused. Activity (#217), lines that never carry text: `read start item=<id> session=<label, else channel id, else direct> chunks=<n>` (` kind=announce` for a session switch announcement), `read end item=<id> finished\|skipped\|failed`, `reader paused` / `reader resumed` (any source: hotkey, CLI, settings page, SDK), `ask kind=<kind> session=<label or channel>`, `hotkey <action>[ <detail>]` (`hotkey mute level=2`, `hotkey flush session=<label>\|announcement session=<label>\|direct\|idle[ scope=session\|all][ others=<labels>]` (`scope` with `agent`; `others`: the sessions whose ready messages scope `all` dropped, comma separated), `hotkey next_channel session=<label>`, `hotkey faster rate=275`, `... failed: <error>`), and from the `system` extension `media pause apps=<names> (reason: reading item=<id>[ session=<label>])`, `media resume apps=<names> (reason: idle\|paused or muted\|disarmed\|mode <mode>\|shutdown)`, `duck apps=<names> level=<n> (reason: ...)`, `restore apps=<names> (reason: ...)`, failures (`media pause failed ...`, `restore failed apps=...`) and `startup sweep: restore\|media resume[ failed] apps=<names>`. Only engages and restores that touched an app are logged. A value with a space is quoted. Troubleshooting (#219), lines that carry text only while `debug_log` is on: `in {json}` for every message received from an authenticated connection (nothing a connection sends before its `hello` is logged, and its refusals at most one line per 10 s) but `get`, `voices` and HTTP `hello` (compact, without the token, credential-looking values `[redacted]`, `options` as their labels, a string over 4 KB clipped; `in failed type=<t> E_<CODE>: <message>` when refused); `agent <message> channel=<id> speak kind=<prose\|question\|permission\|plan\|tool\|summary> entry=<n>[ decision][ waits=<why>] text=...` for text the agent added, `agent <message> channel=<id> <kind>: <why it was not spoken>` (mute level, `skip_code`, held by `read_mode` (`held: <n> chunk(s) wait for minqueue <m> or the turn end`, `held: waits for the turn end (read_mode done), <n> chunk(s)`; `dropped: <n> held chunk(s) (answered\|turn_start\|stop\|flush, read_mode <mode>)` when an answer, a new turn, a stop or a flush drops what was still held; with summaries on, `prose: dropped: <n> chunk(s) kept for the summary (<reason>)`, `summary: cancelled (<reason>): <n> summary in flight, <n> summary waiting for the earlier ones, the settle window` and `<question\|permission\|plan>: dropped: waited for the summary (<reason>)` when an answer or a stop drops them, `<question\|permission\|plan>: spoken now: the summary it waited for was flushed` when a flush releases them, `<prose\|code>: dropped: flushed reply` and `tool: not announced: flushed reply` for the rest of a flushed reply (#228)), the permission prompt of an unanswered question, summaries), `agent <message> channel=<id> dropped: late text ...` (stamped before the last `turn_start`), `agent <message> earcon <kind>`, `agent <message>[ channel=<id>] wipe reason=<turn_start\|answered\|mute\|stop\|flush>`; `drop channel=<id> entry=<n>[ item=<id> (cut while read)][ kind=<k> from=<message>] reason=<why> text=...` for text dropped before it was heard (`replaced by newer text (policy latest)`, `turn_start`, `answered`, `mute`, `stop`, `flush`, `closed`, `muted`); `read text item=<id> session=<s> kind=<k> from=<message> chunks=<read>/<all> text=...` before each `read end`, the exact cleaned text that went to the voice (`kind` also `announce` or `speak`); `read drop item=<id> session=<s> kind=<k> from=<message> unread` for an item the reader dropped unread; `cue text=...` for a spoken cue. Text fields are JSON strings; with `debug_log` false they are left out (and an `in` line keeps only the fixed fields: type, channel, kind, t, index, final, key, a `set` value that is a number, a switch or one of the runtime's own choice settings, ...), so no session text is written |
| `logs\hook.log` | one line per `sonara-hook.exe` call: `hook <Event> pid=<pid> <sent\|started the runtime, sent\|dropped (no runtime answered)\|nothing to send> ms=<n>[ session=<id>][ tool=<name>][ notification=<type>] sent=[messages] payload={raw stdin}` (a string field over 4 KB clipped, credential-looking values `[redacted]`, the `tool_input` of a tool other than `AskUserQuestion` reduced to `{"fields":[names]}` and any `tool_response` omitted, `raw="..."` when stdin is not JSON). With `debug_log` false in `config.json`: `sent=[types]` and no payload |

Files are written atomically (a temp file, then a rename). A `config.json` that is not a JSON object gives the defaults and is copied to `config.json.bad` (and logged) before the next save replaces it. A value out of range is not applied (and logged), the others still apply; it stays in the file, like a key this runtime does not know (from a newer release), until a client sets that key. A saved value the reader refuses at start, such as a voice the current engine lacks (a Kokoro voice from the Python plugin while only OneCore is installed), is logged and kept in `config.json`, so it applies once it is available; the default is used meanwhile. `--engine` on the command line wins over a saved `engine` (except that `--engine fake` keeps a saved external engine, with the fake engine as its fallback), which wins over the default choice below; a saved `engine` may name an external engine; a saved engine that cannot start is logged and the default choice is used. Setting `engine` to another engine replaces a saved voice that engine lacks with the voice in force; setting the same engine again keeps it.

**Migration from the Python plugin.** The first runtime on the default home (`%LOCALAPPDATA%\Sonara`) that has no `config.json` imports the plugin's settings from `%USERPROFILE%\.sonara` (another folder, or another home: `--migrate-from <dir>`): its `config.json` (voice, rate, speech volume, audio mode, duck level, mute level, verbosity, minimum queue, background policy (`earcon_only`, or any other value as `all`), summary mode, command, model, timeout, settle time, style and custom prompts), `keymap.json` (`nav_start` becomes `restart`, `next_session` becomes `next_channel`; only when the home has no `keymap.json`) and `session_prefs.json` (the session `name` becomes `label`; only when the home has none), and the user's own earcons (`config.json` `earcons: {kind: path}`; each file that exists is copied to `earcons\<kind>.wav`, unless one is there; paths to the plugin's bundled WAVs are skipped). Every value of a format 2 file (the plugin saved only what the user set) is saved, also one equal to the runtime's default, so a user who chose the plugin's old default keeps it; in a file from before format 2 (every key written) a value equal to the plugin's old default (rate 200, volume 100, `audio_mode` `"off"`, `mute_level` 0, `verbosity` `"everything"`, `minqueue` 1, `background_policy` `"earcon_only"`) counts as unset. Summary fields equal to the runtime's default are not saved; a Chatterbox voice speaks as `af_heart`, `audio_control: true` is `audio_mode: duck`, a speech volume above 100 is 100, and in a file from before the plugin's format 2 the old defaults `duck_level: 20` and `summary_timeout: 20` count as unset. The cue voice and fast cues are not imported (the runtime speaks its control cues in the voice in force). The plugin's folder is only read. The migration writes `config.json` with the `_migrated` marker, so it runs once; what it did is in `logs\sonarad.log`.

## Testing aids

`sonarad --engine kokoro|onecore` picks the engine to start with; without it, the saved `engine`, else `kokoro` when `onnxruntime.dll` is next to `sonarad.exe` (`SONARA_ORT_DYLIB` names another copy, a development aid), else `onecore`. A Kokoro start verifies or downloads and loads the model in the background right away. `sonarad --engine fake` uses a deterministic tone engine (10 ms of audio per character at rate 200; text containing `[fail]` fails to synthesize) and, unless `--output device` is given, `--output null`: a silent output that keeps real time. `sonarad --system fake` replaces the Windows side of the [`system`](#extension-system) extension with fake apps, media sessions, hotkeys and keyboard layout kept in `<home>\fake-system.json` (read and written on every operation), so a test can set up the "apps", kill the runtime and check what the next one restores. `sonarad --keys fake` keeps external engine keys in `<home>\fake-keys.json` instead of Windows Credential Manager (`--keys windows`, the default), so tests never touch the user's credentials. They are meant for tests and the conformance suite, not for apps. With `--engine fake` there is no Kokoro engine, so nothing is ever downloaded.

`sonarad --no-external-engines` refuses [external engines](#external-engines): no `engines` capability, every `engine_*` message is `E_UNSUPPORTED`, and `engines.json` is not read. A host that bundles Sonara and must not send text off the PC starts it this way.

## Examples

With `runtime.json` read into `$PORT`, `$HTTP_PORT` and `$TOKEN`:

```sh
curl -s -H "Authorization: Bearer $TOKEN" -d '{"text": "Hello from curl."}' http://127.0.0.1:$HTTP_PORT/v1/speak
curl -s -H "Authorization: Bearer $TOKEN" -d '{"action": "pause"}' http://127.0.0.1:$HTTP_PORT/v1/control
curl -sN -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:$HTTP_PORT/v1/events?events=state,items"
```

Python, standard library only: speak and follow the item to its end.

```python
import json, os, socket

home = os.environ.get("SONARA_HOME") or os.path.join(os.environ["LOCALAPPDATA"], "Sonara")
rt = json.load(open(os.path.join(home, "runtime.json"), encoding="utf-8"))
sock = socket.create_connection(("127.0.0.1", rt["port"]))
f = sock.makefile("rw", encoding="utf-8", newline="\n")

def send(msg):
    f.write(json.dumps(msg) + "\n"); f.flush()

send({"type": "hello", "token": rt["token"], "client": {"name": "example", "version": "1"}})
send({"type": "subscribe", "events": ["items"]})
send({"type": "speak", "text": "Hello from Python.", "id": "s1"})
for line in f:
    msg = json.loads(line)
    print(msg)
    if msg.get("event") == "item" and msg["phase"] in ("finished", "skipped", "failed"):
        break
```

## External engines

Protocol 1.2, capability `engines` (runtime 0.15.0, #224; design `docs/plans/2026-10-04-external-engines-spec.md`). The user adds speech engines that are not part of Sonara: a cloud API or a local server that speaks OpenAI's speech API. Each is a **profile** that becomes an engine next to `kokoro` and `onecore` (licence class `external`), chosen with `set engine` like any other. Adding one sends nothing anywhere: text goes to it only while it is the current engine (or in `engine_test`, a voice list). The messages are core (no extension to enable) and work over TCP and HTTP (`POST /v1/engine_add`); a runtime started with `--no-external-engines` answers each with `E_UNSUPPORTED` (`this runtime does not allow external engines`).

**Never silent.** When the provider cannot speak a sentence (no key, a refused key, no credit, rate limited, unreachable, a timeout, a server error, an unknown voice, bad settings, audio that is not WAV or PCM), the runtime speaks that sentence with its built-in engine (Kokoro, which itself falls back to OneCore) and, once per episode, prepends a short cue: `<label> cannot be reached. Reading with the built-in voice.` (`has no key`, `refused the key`, `is out of credit`, `is busy`, `has a server problem`, `does not know this voice`, `settings do not work`). An episode ends with a success or a change of the profile or key. `state.engine_status` of the engine shows the `reason` and the `fallback`: `waiting` while it will try again by itself (the breaker after two transient failures in a row: 30 s, doubling to 300 s, with no network wait meanwhile; `quota`: 10 minutes), `unavailable` while the user must change something (`no_key`, `auth`, `bad_voice` for that voice, `bad_config`, `format`). A sentence goes to the provider once (a 429 or 503 with a `Retry-After` of at most 1.5 s is tried once more). For an engine with a slow round trip the reader synthesizes more sentences ahead (`options.prefetch`: 2 for a cloud engine, 1 for a local one), and short repeated texts (spoken cues) are kept in memory.

**Keys.** A key goes in only through `engine_add` (`secret`) or `engine_key` and is kept in Windows Credential Manager (generic credential `sonara:<id>`, this user on this PC), or read from an environment variable of the runtime's process (`key_ref` `env:NAME`). It is never in a file of the home, a log line or a reply, and goes only in the provider's authentication header, over HTTPS or to a loopback host. The `in` lines of the troubleshooting log drop the `secret` field entirely.

### Profile

| field | rule |
|---|---|
| `id` | 1 to 32 of `a-z`, `0-9`, `-`, `_`, starting with a letter or digit; not `kokoro`, `onecore`, `fake`, nor starting with `sonara` |
| `kind` | `openai-compatible` in this runtime (`engine_list.kinds`); a profile of another kind (from a newer release) is kept and listed with `supported: false` |
| `label` | 1 to 40 characters; spoken in cues. Default: the preset's name |
| `url` | the base URL with the API version, e.g. `https://api.openai.com/v1`, `http://127.0.0.1:8880/v1`. HTTPS, or plain HTTP to a loopback host (`localhost`, `127.0.0.0/8`, `::1`), or with `options.allow_http` to a server on the network (then never with a key); no user, query or fragment |
| `model`, `voice` | up to 200 characters; the voice is used when the reader's voice is `null` |
| `key_ref` | `"none"`, `"credman"` or `"env:NAME"`, where `NAME` ends in `_API_KEY` or `_SPEECH_KEY`, is `SPEECH_KEY`, or starts with `SONARA_` (case-insensitive; any other variable is `E_BAD_REQUEST`, so a client cannot send the runtime's other secrets to a server it chose). Default: `none` for a loopback server, `credman` otherwise |
| `options` | per kind, below; an unknown option is `E_BAD_REQUEST` |

At most 16 profiles. Options of every kind: `timeout_ms` (1000 to 120000; 15000 for a cloud engine, 60000 for a local one), `prefetch` (1 to 4), `allow_http` (boolean). Options of `openai-compatible`: `preset` (below, default `generic`), `response_format` (`wav`, the default, or `pcm`), `sample_rate` (8000 to 48000; raw PCM, and sent to Speaches), `instructions` (for preset `openai` sent only to `gpt-4o-mini-tts` models; never sent to `kokoro-fastapi` or the Chatterbox presets; sent as set to any other server, such as LocalAI's expressive backends), `extra` (an object merged into the request body last, for a server's own fields), `voices_path` (the voice list path of a `generic` server).

| preset | default model | default voice | voice list |
|---|---|---|---|
| `openai` | `gpt-4o-mini-tts` | `marin` | the 13 OpenAI voices (9 for `tts-1`, `tts-1-hd`); default URL `https://api.openai.com/v1` |
| `kokoro-fastapi` | `kokoro` | `af_heart` | `GET {url}/audio/voices`; sends `stream: false` |
| `localai` | (required) | none | `GET {url}/audio/voices?model=<model>` |
| `speaches` | (required) | `af_heart` | `GET {url}/audio/voices`; sends `sample_rate` |
| `openedai-speech` | `tts-1` | `alloy` | the six OpenAI voices |
| `chatterbox-api` | `chatterbox` | `alloy` | `GET {root}/voices` (`{root}`: the URL without `/v1`) |
| `chatterbox-server` | `chatterbox` | (required: a file name such as `Emily.wav`) | `GET {root}/get_predefined_voices`; always WAV |
| `generic` | `tts-1` | `alloy` | `GET {url}/audio/voices` (or `voices_path`); a failure is an empty list |

A sentence is `POST {url}/audio/speech` with `{model, input, voice, response_format, speed}` (`speed` is the rate / 200, from 0.25 to 4.0) and `Authorization: Bearer <key>` when there is a key. The answer is WAV (any rate, 16-bit or float) or raw 16-bit mono PCM (rate from the `Content-Type` `rate=`, else `sample_rate`, else 24000); MP3, Ogg, FLAC, JSON or HTML with a 200 status is `format`.

**The profile view** (in replies; never a key): the profile with the values in force (the preset's defaults filled in), plus `key_present` (a key resolves now, also for `env:`), `sends_text_to` (the URL's host), `local` (a loopback host), `license_class: "external"`, `supported`, `current` (it is the reader's engine) and `status` (its `engine_status` without `engine`; `null` when not usable). An entry that cannot be used has `error`.

```json
{"id": "openai", "kind": "openai-compatible", "label": "OpenAI", "url": "https://api.openai.com/v1", "model": "gpt-4o-mini-tts", "voice": "marin", "key_ref": "credman", "options": {"preset": "openai"}, "key_present": true, "sends_text_to": "api.openai.com", "local": false, "license_class": "external", "supported": true, "current": false, "status": {"ready": true, "status": "ready"}}
```

### Messages

A request's `id` is its correlation id (echoed in the reply), so these messages name a profile with `engine`.

| message | fields | reply |
|---|---|---|
| `engine_list` | none | `{engines: [view...], builtin: ["kokoro", "onecore"], kinds: ["openai-compatible"], presets: [...]}` |
| `engine_add` | `engine` (the profile), `secret?` (stored as its key; sets `key_ref` `credman` when absent, `E_BAD_REQUEST` with `env:` or `none`), `replace?` (default `false`) | `{engine: view}`. Validates, saves `engines.json`, stores the key, registers the engine; it does not select it. An existing id without `replace: true` is `E_BAD_REQUEST`; a replace keeps the stored key unless `secret` is given, forgets the engine's failures and cached audio, and applies to the next sentence when it is the current engine |
| `engine_remove` | `engine`, `forget_key?` (default `true`) | `{removed: "<id>", engine: "<engine now in force>"}`. The current engine is first switched to the default choice (Kokoro when installed, else OneCore; saved like any `set`); then the engine is unregistered, removed from `engines.json` and its stored key deleted |
| `engine_key` | `engine`, `secret` (a string, or `null` to delete) | `{engine, key_present}`; clears a `no_key` or `auth` block. `E_BAD_REQUEST` for a profile whose `key_ref` is `env:` or `none` |
| `engine_test` | `engine`, `text?` (default "Hello. This is how Sonara sounds with this voice.", at most 300 characters), `voice?`, `play?` (default `true`) | `{engine, voice, ms, sample_rate, duration_ms}`. One synthesis at the current rate with no fallback and no cache, played as a clip over whatever is read (as `preview`) when `play`. A success clears the engine's blocks and breaker; a failure is `E_ENGINE` with `reason` |

```json
> {"type": "engine_add", "engine": {"id": "openai", "kind": "openai-compatible", "options": {"preset": "openai"}}, "secret": "sk-...", "id": 1}
< {"id": 1, "ok": true, "engine": {"id": "openai", "kind": "openai-compatible", "url": "https://api.openai.com/v1", "model": "gpt-4o-mini-tts", "voice": "marin", "key_ref": "credman", "key_present": true, "sends_text_to": "api.openai.com", "current": false, ...}}
> {"type": "set", "key": "engine", "value": "openai", "id": 2}
< {"id": 2, "ok": true, "key": "engine", "value": "openai"}
> {"type": "engine_test", "engine": "openai", "play": false, "id": 3}
< {"id": 3, "ok": false, "error": {"code": "E_ENGINE", "message": "OpenAI refused the key (401): Incorrect API key provided", "reason": "auth"}}
```

| error | code |
|---|---|
| a profile or message field breaks a rule | `E_BAD_REQUEST` |
| an unknown profile id | `E_NOT_FOUND` |
| the runtime refuses external engines; a kind this runtime lacks | `E_UNSUPPORTED` |
| Credential Manager failed; `engine_test` failed | `E_ENGINE` |

The command line: `sonara engines list|add|key|use|test|remove` (`sonara engines help`); a key is read from stdin or a prompt without echo, never from an argument. The SDKs: `client.engines` (`@sonara/client` `EnginesApi`, `sonara-client` `Engines`).

## Extension `channels`

Spec section 4.2, L2 (`crates/sonara-channels`). Several named sources (terminal tabs, chats) share the one reader: each **channel** keeps its own messages and policy, and one channel is read at a time. Enable it with `hello` `extensions: ["channels"]`. Black-box tests: `conformance/channels/`.

**Model.** A channel holds its current **batch**: the messages sent to it since it was last caught up, with a read position. Messages wait in their channel and go to the reader one at a time, only when the reader is idle, so text spoken without a `channel` (core `speak`) is read first. Heard messages stay, so a manual return can replay the batch; a message sent to a channel that is caught up (and not still reading its last message) starts a new batch.

- **Policy** `latest` (the default): a new message replaces the channel's unread messages, so the newest one is always read and never dropped ("one message, always the last"). `queue`: every message is read, in order.
- **Who reads next:** the channel being read keeps the floor until its batch is read; then the focused channel; then the first channel (in opening order) with something unread. A channel you left with `next_channel` is not resumed on its own until it gets a new message.
- **Muted channels** (`channel_prefs` `muted`): a muted channel's messages wait, unread, and it never takes the floor (muting the channel being read cuts its item); `next_channel` skips it unless every channel is muted. Unmuted, its waiting messages are read.
- **Announcements:** a switch to another channel is announced by a short item before its first message: `"<label>."`, or `"<label>, reading again."` when the batch is replayed from the top. An automatic hand-off is announced when the channel differs from the one that read last (never for the first channel to read); `next_channel` is always announced. A channel without a `label` is not announced. `set channel_announce "off"` turns announcements off. With the `agent` extension on (an agent host such as the Claude plugin) the texts are the Python plugin's, `"Session changed: <label>."` and `"Session changed: <label>, reading again."`, and every announcement is preceded by the `session_change` earcon, played as the announcement is handed to the reader (so the chime is heard first), for automatic hand-offs and manual switches alike (#209); not at `mute_level` 2. Without `agent` the texts stay generic.

### Messages

| type | fields | effect |
|---|---|---|
| `channel_open` | `channel`, `label?`, `host_tab?`, `policy?: latest\|queue` | open a channel, or update an open one (its messages stay; `policy` omitted keeps the current one). Reply `{channel, created, policy}` |
| `channel_close` | `channel` | close it and forget its messages; if it is being read, its item is cut and the next channel follows (not announced) |
| `focus` | `channel` | read this channel next once the channel being read has finished its batch (does not cut) |
| `speak` | `channel?` plus the core fields | with `channel`: add a message to it (opened with the defaults if needed). `mode` overrides the policy for this message (`replace` drops the channel's unread messages, `append` keeps them). `interrupt: true` reads it now: it goes before the channel's unread messages, the current item is cut and the switch is announced; another channel's message cut this way is read again once this channel's batch is read. Reply `{item_id, channel, dropped}`: `item_id` is the reader item when the message went to the reader at once, `null` while it waits in its channel (behind other messages or an announcement); `dropped` counts the unread messages it replaced |
| `control` | `channel?` plus the core `action`, or `action: next_channel` or `flush` | see below |

`channel` is a non-empty string (`E_BAD_REQUEST` otherwise); an unknown channel in `channel_close`, `focus` or `control` is `E_NOT_FOUND`.

**`control` once `channels` is enabled.** Without `channel` the actions are the core ones, except:

- `stop` also skips every channel to its end (nothing more is read until a new message; heard messages stay replayable).
- `restart` while nothing is playing replays the batch of the channel being read or read last (the Claude plugin's Up key), not only its last item.

With `channel`: `stop` skips that channel to its end and cuts its item if it is being read; `restart` goes back to the start of its item if that channel is being read, else replays the channel's batch from the top and switches to it (cutting the current item, announced); any other action applies only while that channel is being read, and is a no-op otherwise.

`next_channel` (reply `{channel}`, `null` when no channel is open) switches now: it moves around the channels in opening order, skipping channels with nothing to hear (unless all are empty), starting from the channel being read or the one that read last. It cuts the current item and announces the target. A fully heard target, landing on the same channel, or returning to a replay in progress replays the batch from the top; unread messages resume where they stopped (a message cut by the switch is read again).

`flush` (#228, the flush hotkey; reply `{flushed, channel, scope, others}`) stops what is being read now. When a channel's item is playing or paused, that channel is skipped to its end and its item cut, as `stop` with that `channel` (`flushed: "channel"`, `channel` its id); a paused reader is un-paused. In scope `session` the other channels keep everything and are read next as usual. When a switch announcement is playing, only the announcement is skipped and the channel it names is read at once with all its messages (`flushed: "announcement"`, `channel` the announced one): the press was aimed at the channel that had just ended. Once that channel's first item has started, a flush flushes it. When the reader is reading text spoken without a `channel`, only that item is skipped (`flushed: "direct"`). When nothing is being read, `flushed` is `"nothing"`: in scope `session` nothing changes; with `agent` and scope `all` the other sessions' ready text is still dropped and listed in `others`. It takes no `channel` (`E_BAD_REQUEST`; use `stop` with the channel). Without the extension it is `E_UNSUPPORTED`. `scope` is the `flush_scope` in force (`"session"` without `agent`) and `others` the channels whose ready messages scope `all` dropped too (`[]` in scope `session`). With `agent` it also drops that session's agent state and skips the rest of its reply, and scope `all` drops the other sessions' ready messages, see below.

A switch (`next_channel`, `restart` with a channel, `speak` with `interrupt`) only cuts the current item: text spoken without a `channel` that is already waiting in the reader still plays first, so it comes between the announcement and the channel's message. A channel item ended from outside the extension (a core `speak` with `interrupt`, or `skip`) counts as heard: the extension cannot tell it apart from a user skip, so that message is not read again on its own (`restart` with the channel replays it).

### Setting

| key | value |
|---|---|
| `channel_announce` | `"on"` (default) or `"off"`: switch announcements |
| `channel_prefs` | the user's preferences per channel, for a settings page. `get`: a list of `{channel, open, reading, client_label, host_tab, label, voice, muted}`, the open channels first (in opening order), then channels with saved preferences only (most recent first). `set {channel, label?, voice?, muted?}` changes the fields given (`null` or `""` clears a label or voice) and replies with the list. A `label` replaces the one the client sends in `channel_open` (also for a channel opened by its first text), so switch announcements say it; `client_label` is the client's. `muted: true` mutes the channel at once and in every later run (see Muted channels, #196); `voice` is saved for the page and not applied yet. `set {channel, forget: true}` forgets a channel that is not the focused one (`E_BAD_REQUEST`): its preferences, and its messages and turn (it is closed if open; a session that died without ending) |

### State

`state.now_playing` gains `channel` and `host_tab` (both `null` for text spoken without a channel; an announcement belongs to the channel it announces), and `queued` also counts the channels' unread messages. A `state` event is sent when the reader's state changes, so `queued` catches up with a new channel message at the next change.

```json
{"event": "state", "seq": 31, "now_playing": {"item_id": 12, "label": "Build tab", "text": "Build finished.", "chunk": 0, "chunks": 1, "channel": "tab-3", "host_tab": "3"}, "queued": 1, "paused": false, "muted": false, "volume": 100, "rate": 200, "voice": null, "engine_status": {"engine": "onecore", "ready": true, "status": "ready"}}
```

## Extension `agent`

Spec section 4.3, L3 (`crates/sonara-agent`). Speech for coding agents and chat assistants on top of [`channels`](#extension-channels): one channel per agent session, with streamed text, turns, decisions spoken with priority, earcons, three mute levels and optional summaries. Enable it with `hello` `extensions: ["agent"]`; it needs `channels`, which is enabled with it (the reply lists both). Black-box tests: `conformance/agent/`.

**Model.** Each channel has a current **turn**. The agent's text is streamed into it (`stream`), split into sentences and added to the channel's batch as it completes, whatever the channel's policy (a turn is many messages). A new turn (`turn_start`) drops what is left of the previous one: its unread sentences, and its item if it is being read ("one message, always the last"). Text that arrives late from an earlier turn is dropped (see `t`). Decisions (`ask`) are read before the other channels as soon as the item playing ends (the batch reading now waits), and play an earcon.

**Background sessions (`background_policy`, #195).** With `"earcon_only"` (the Python plugin's default; the runtime's is `"all"` since #202) only the focused channel (the session the user prompted last: the Claude hooks `focus` on every prompt) is read automatically; the other channels play their earcons, and their text and decisions wait until the user prompts that session (`focus`), switches to it (`next_channel`) or replays it (`restart`). Exceptions, as in the Python plugin: the channel focused before keeps the right to finish what it had unread when the focus moved, a summary (or the raw text standing in for one) is read whatever the focus, and text a host speaks into a channel (`speak` with `channel`) is always read. With no channel focused nothing is held back. `"all"` reads every channel in turn.

**Dead sessions.** A channel with no agent message for 6 hours has its turn state freed (as `channel_close` does for the turn); its channel is closed too when it is neither focused nor being read and has nothing unread. `channel_prefs` `forget` does it at once.

**Late text (`t` and `turn`).** Senders that run as separate processes (hooks) can deliver the old turn's last text after the new prompt. Every agent message may carry `t`, the sender's start time in seconds (any clock, the same one for all senders of a channel, such as Unix time). A `stream`, `turn_start` or `turn_end` whose `t` is older than the channel's last accepted `turn_start` is dropped and answered `{stale: true}`; so is one naming, in `turn`, a turn id that a later `turn_start` replaced. Messages without `t` and `turn` are never stale.

### Messages

| type | fields | effect |
|---|---|---|
| `stream` | `channel`, `delta`, `index?` (default 0), `final?`, `turn?`, `t?` | a piece of the agent's text. `index` numbers the deltas of one block (a new block may restart at 0); `final` ends the block and flushes an unfinished sentence. Reply `{stale}` |
| `turn_start` | `channel`, `turn?`, `t?` | a new turn: the channel's unread text is dropped and its item cut; a question waiting for an answer and summary work of the old turn are dropped. If the channel is the one being read or read last and the reader is paused, it resumes; **a new turn in another channel keeps the pause on**. Reply `{stale}` |
| `turn_end` | `channel`, `turn?`, `t?` | the agent finished: plays `turn_done` and reads text held by `read_mode`. Reply `{stale}` |
| `ask` | `channel`, `kind: question\|permission\|plan`, `text?`, `options?`, `multi_select?`, `notes?`, `hint?`, `hint_once?` | a decision, read with priority (after the item playing, before the other channels). `question`: the text, then `Option n: label.` and its description for each of `options` (strings or `{label, description?}`; an option without a label keeps its number), plays `choice`, and marks the channel as waiting for an answer. `permission`: the pending action, plays `permission`; while a question waits, the permission prompt it fires itself is dropped (no earcon, no text) and clears the mark. `plan`: `"Plan ready. <text>"`, no earcon. `notes` is read after the decision; `hint` too at verbosity `everything`, and `hint_once` after it the first time a channel gets one |
| `tool` | `channel`, `name`, `summary?` | the agent runs a tool: clears a waiting question; at verbosity `everything` it reads `summary` (else `"Running <name>."`) after the text held so far (read_mode `done`: the text stays held until the turn ends or a decision) |
| `answered` | `channel` | the user answered the question: everything queued for the channel is stale, so its unread text is dropped and its item cut, summary work and held decisions are dropped; the turn goes on |
| `earcon` | `kind` | play an earcon: `choice`, `permission`, `error`, `turn_done`, `nav`, `nav_edge`, `session_change`, `summary_failed` |

`channel` is a non-empty string (`E_BAD_REQUEST` otherwise); a channel is opened with the defaults when it gets its first text (open it with `channel_open` to give it a label for announcements). With the extension on, `channel_close` also forgets the channel's turn, and `control stop` without a channel also drops every channel's summary work and held decisions. `control flush` (#228) does that for the session being read: its prose held by `read_mode`, the prose kept for its summary, its summaries in flight, waiting or settling and the decisions waiting for them are dropped, and the rest of that reply is skipped: until the session's next `turn_start`, its prose (also late prose after its `turn_end`) is dropped and its tool runs are not announced (logged `dropped: flushed reply`), and it makes no summary. Its decisions (question, permission, plan) asked later in that reply are still read, since they need an answer; `turn_done` still plays when it ends. A flush answers nothing: a question of that session keeps its awaiting mark (its permission prompt stays silent), and the decisions that waited for the cancelled summary are read now instead of being dropped (decisions already queued in the channel are dropped with its other unread text). With `flush_scope` `"session"` every other session keeps its held prose, its summary work and its queued text, and is read next. With `"all"` every other session whose turn has ended (its `turn_end` came since its last `turn_start`) also loses its ready messages: its unread text, its summaries in flight, waiting or settling (the decisions that waited for them are read now), and a channel without agent turn state (text a host spoke into it) loses its unread text too, also when nothing was being read (text the background policy or a session mute holds). When anything of such a session was dropped, the late prose of that reply (after its `turn_end`) is skipped too, as for the session being read; its next reply is read as usual. In both scopes a session still writing its reply (no `turn_end` yet) keeps everything: its queued and held text and its summary work, read when done. A reply counts as still being written until its `turn_end`: a turn the user interrupted (Claude Code fires no `Stop` for it) keeps its text in scope `all` until that session's next `turn_start`. Mute (`mute_level`) is the way to silence every session.

Earcons are mixed over the speech (they never pause or cut it) and follow the output volume.

#### Custom earcons

`sonarad` plays `<home>\earcons\<kind>.wav` (for example `session_change.wav`, `turn_done.wav`) instead of the bundled clip of that kind (#209). Any RIFF/WAVE file works: 8-, 16-, 24- or 32-bit integer PCM or 32- or 64-bit float, plain or `WAVE_FORMAT_EXTENSIBLE`, mono or stereo (mixed down), any sample rate (the output resamples), up to 10 seconds and 16 MB. A file is checked again (size and modification time) each time its kind plays and on `get earcons`, so adding, replacing or deleting one applies at the next play, without a restart or a reload command. A file that cannot be used (not a WAV, an unsupported format, silent, too long or too big, unreadable) plays the bundled clip instead, and one line in `logs\sonarad.log` says why (once per version of the file). The `earcon` event names the kind either way.

### Settings

| key | value |
|---|---|
| `mute_level` | `0` (default), `1`: agent text is not spoken (what is queued and playing is dropped), earcons still play; `2`: earcons are silent too. Setting 1 or 2 stops everything queued and playing, core `speak` and channel `speak` items included (as the Python daemon's global mute); text spoken without the extension afterwards is read as usual |
| `verbosity` | `"everything"`: text, decisions, tool announcements, hints, and each code block announced as `"<n>-line <lang> code block"`; `"skip_code"` (default): text and decisions, code blocks dropped without a word, no tool announcements or hints. Aliases for older clients (#214): `"medium"` and `"quiet"` are `"skip_code"`, `"all"` is `"everything"`; a `set` with an alias answers (and saves) the new name, and a saved alias loads as the new name |
| `read_mode` | when a turn's sentences are read (#222): `"immediate"`: each as it arrives; `"queue"`: held until `minqueue` are waiting, the turn ends, a tool runs or a decision arrives; `"done"` (default): held until the turn ends or a decision (question, permission, plan) arrives, a tool run does not release them. A decision reads the held sentences first, then itself; after the turn ends, late sentences are read at once. With summaries on, summaries own the turn whatever the mode. A `config.json` from before #222 with a `minqueue` the user set and no `read_mode` loads `0` and `1` as `"immediate"` and more as `"queue"` (written with the next change); without one the default applies |
| `flush_scope` | what `control flush` (the flush hotkey) skips (#228): `"session"` (default): the session being read, for the rest of its reply, then the next session is read; `"all"`: also every other session's ready messages (queued text, finished replies, summaries); a reply still being written is kept in both. See `control flush` above |
| `minqueue` | `0` to `10` (default 5), read mode `"queue"` only: a turn's sentences are held until this many are waiting, the turn ends, a tool runs or a decision arrives; `0` and `1` read at once |
| `background_policy` | `"all"` (default) or `"earcon_only"`: see Background sessions |
| `summaries` | `{enabled, command, model, timeout, settle_ms, style, prompt, prompts, default_prompts}`: see below. `set` merges the fields given; `get` returns them all |
| `earcons` | read-only (`set` is `E_BAD_REQUEST`): `{folder, kinds, custom}`: `folder` is the custom earcons folder (`null` when the runtime has none), `kinds` every earcon kind, `custom` the kinds a usable file there replaces now. See Custom earcons |

**Summaries** (off by default). The turn's text is recorded instead of read; when the turn ends and no text came for `settle_ms` (0 to 5000, default 600), a headless agent writes a spoken recap of it: `command` `"claude"` (`claude -p`, tools and settings off) or `"codex"` (`codex exec`, read-only), `model` (default `"haiku"`), `style` `"tidy"`, `"natural"` (default) or `"brief"`, or a custom `prompt`. The command is found on `PATH` only and runs in the user's home folder with no window; past `timeout` seconds (15 to 300, default 60) it is killed with its child processes. A turn shorter than 280 characters is read as it is. A summary that fails or comes back empty falls back to the turn's text. A decision waits for the recap of the text before it (read first), at most `timeout` + 5 s. Recaps are read in the order the turns ended, and one still out after twice `timeout` is read as plain text. A new turn, an answer, `stop` or a `flush` of that session drops the recaps of the channel still out. A runtime built without the summarizer answers `enabled: true` with `E_UNSUPPORTED`.

**Custom prompts.** Each style can have its own instruction: `prompts` is `{style: text}` (`set` changes the styles given; `null` or a blank text goes back to the built-in one), and `prompt` is the custom instruction of the style in force (`set` with `prompt` changes that style's). Before #201 `prompt` was one instruction for every style; no runtime with that meaning shipped, so the protocol minor was not bumped for it. `get` also returns `default_prompts`, the built-in instruction of each style (read-only), so a page can show and edit it.

### Events

Stream `earcons` (ask for it by name in `subscribe`; `E_UNSUPPORTED` while the extension is off): `{"event": "earcon", "kind": "turn_done"}` for every earcon played.

```json
> {"type": "hello", "token": "...", "extensions": ["agent"]}
< {"ok": true, "extensions": ["channels", "agent"], ...}
> {"type": "turn_start", "channel": "s1", "t": 1759400000.25}
< {"ok": true, "stale": false}
> {"type": "stream", "channel": "s1", "delta": "Done. All tests pass.", "index": 0, "final": true, "t": 1759400003.5}
< {"ok": true, "stale": false}
> {"type": "ask", "channel": "s1", "kind": "question", "text": "Deploy now?", "options": [{"label": "Yes"}, "No"]}
< {"ok": true}
```

## Extension `system`

Spec section 4.4, L4 (`crates/sonara-system`, Windows). What happens to other apps' audio while Sonara speaks, global hotkeys, spoken control cues and the settings page. Enable it with `hello` `extensions: ["system"]`. Black-box tests: `conformance/system/` (with `--system fake`).

**Armed while needed.** Like every extension, `system` is enabled for the whole runtime once a client asks for it, and its keys and the settings page work from then on. It acts on the PC (ducks or pauses other apps, holds the hotkeys) only while it is **armed**: while a TCP client whose `hello` asked for it is connected, or for good once a client asked for it with `keep_alive: true` (over TCP or HTTP). When the last client that needed it disconnects, other apps are restored at once and the hotkeys are released, even if the reader goes on reading. A plain HTTP request (the settings page) never arms it.

**Other apps' audio.** With `audio_mode` `duck`, every other app's audio session on every active output device is lowered to `duck_level` percent while an item is being read (playing, not paused, not muted); with `pause`, media apps that are playing (Windows media transport controls) are paused and later resumed. Never touched: the runtime's own process, the Windows audio engine (`audiodg.exe`) and virtual mixers whose session is the whole mix (SteelSeries Sonar, VoiceMeeter); an app already at or below the level is left alone. Other apps come back at once on `pause`, `mute`, a mode change and when the extension is disarmed, about 0.4 s after the reader goes idle (so the gap between two messages does not bring them up and down), and when the runtime exits.

**Crash restore.** Before an app is lowered or paused it is recorded in `state\duck_state.json` or `state\pause_state.json` in the home, and the files are removed once everything is back. A runtime that starts finds these files and restores the apps before it accepts clients; what still fails stays recorded and is retried by the next restore.

### Settings

| key | value |
|---|---|
| `audio_mode` | `"off"`, `"duck"` or `"pause"` (default) |
| `duck_level` | integer 0 to 100 (default 30): the volume other apps keep while ducked. A change applies at once while ducked |
| `hotkeys` | `get`: the keymap (below). `set`: `{"action", "key", "mods"}` binds an action, `{"action", "key": null}` unbinds it, `"reset"` restores the defaults; the reply is the keymap now in force |
| `settings_url` | read-only (`set` is `E_BAD_REQUEST`): `http://127.0.0.1:<http_port>/settings?token=<token>`, the settings page |
| `runtime` | read-only: `{pid, uptime_s, http_port, config, previews, saved_voice, engine_status}`; `config` is the path of the saved settings (`null` when the runtime saves none), `previews` whether `preview` works, `saved_voice` the voice saved in `config.json` (`null`: none), which differs from `voice` while the engine lacks it, `engine_status` the same object as in `state` (#214, for clients that poll) |

These settings are saved like the others (see [Saved settings](#saved-settings)).

### Shutdown

`{"type": "shutdown"}` (#202) ends the runtime: what is reading or queued stops, later requests are `E_BUSY`, and once the reply `{ok: true}` went out the process restores other apps' audio, releases the hotkeys and exits, like an accepted takeover. It is how `sonara stop`, `sonara uninstall` and an upgrade (`sonara start` of a newer release) end the Claude plugin's runtime. Before a client enabled `system` it is `E_UNSUPPORTED`.

### Voice previews

`{"type": "preview", "voice"?: "<id or name>", "text"?: "<text>"}` says a short sample (default: `"Hello. This is how Sonara sounds with this voice."`, at most 300 characters) with a voice of the current engine (default: the voice in force) at the current rate, and replies `{engine, voice}` once it is playing. The sample is synthesized on engines of its own (its own `onecore`; Kokoro's loaded model is shared with the reader, so a Kokoro sample waits at most for the sentence being synthesized, and a skip on the reader cancels it) and played as a clip mixed over whatever is being read, like an earcon: nothing is paused, cut or queued again, and the output volume applies (a muted reader plays it silently). An unknown voice is `E_NOT_FOUND`; before a client enabled `system` it is `E_UNSUPPORTED`.

### Spoken cues

Short confirmations, as the Python plugin spoke them (#197): the hotkeys say `"Paused."` / `"Resumed."` (pause, only while an item is loaded), `"Muted."`, `"Super muted."`, `"Unmuted."` (mute cycle; without `agent`, `"Muted."` / `"Unmuted."`), `"Rate 225."` (faster, slower) and `"No session."` (`next_channel` with no channel open); a `set` of `mute_level` that changes it, of `audio_mode` (`"Audio off."`, `"Audio ducking."`, `"Media pause."`) and of `duck_level` (`"Duck level 40 percent."`) say theirs whoever sent it. A `rate` set from a page is not announced. A cue is synthesized on the extension's own engines in the voice and at the rate in force and played as a clip mixed over whatever is read, like a voice preview: it is heard while the reader is paused and at every `mute_level`, never touches the queue, and a muted reader (core `mute`) plays it silently. Cues are spoken in order; a rate, duck-level or audio-mode cue still waiting when a newer one of the same kind comes is skipped. Stream `cues` (ask for it by name in `subscribe`, or `GET /v1/events?events=cues`; `E_UNSUPPORTED` over TCP while the extension is off): `{"event": "cue", "text": "Muted."}` for every cue spoken. The Python plugin's setup-guide cue ("run slash sonara install") is not carried over: the runtime needs no install step (the hook starts it).

### Hotkeys

Actions and what they do (the same as the protocol request named):

| action | default | effect |
|---|---|---|
| `restart` | Ctrl+Alt+Up | `control restart` |
| `flush` | Ctrl+Alt+Down | with `channels`: `control flush` (the session being read and the rest of its reply, #228; with `flush_scope` `all` also every other session's ready messages); without it, `control stop`. With `agent`, plays `nav` (or `nav_edge` when nothing was dropped) |
| `pause` | unbound | `control toggle`, then the cue `"Paused."` or `"Resumed."` |
| `mute` | Ctrl+Alt+M | with `agent`: `mute_level` 0, 1, 2, 0 and so on; else `mute` and `unmute`; then its cue |
| `next_channel` | Ctrl+Alt+P | `control next_channel` (with `channels`); with `agent`, plays `session_change` then says `"Session changed: <label>."` (the announcement; when switches are not announced, the earcon alone); with no channel open, the cue `"No session."` |
| `faster` / `slower` | unbound | `rate` plus or minus 25, then `"Rate N."` |

A binding is a `key` (a letter, a digit, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `period`, `leftbracket`, `rightbracket`) and `mods` (`ctrl`, `alt`, `shift`, `win`). A hotkey must hold Ctrl, Alt or Win (`E_BAD_REQUEST` otherwise: it would take that key away from every app); an unknown key, modifier or action is `E_BAD_REQUEST`, and nothing is written. Holding a key does not repeat an action, and a second press of `pause` or `mute` within 0.3 s is ignored. Hotkeys do nothing once a takeover was accepted.

The defaults use Ctrl+Alt, which is AltGr on many European keyboard layouts: a hotkey that is AltGr typing a character is reported in `altgr`, and the fix is a binding with Win (Windows itself owns Win+Alt+Up/Down/M/P). The user's bindings are kept in `keymap.json` in the home (only the overrides; `nav_start` and `next_session` from the Python plugin's keymap are read as `restart` and `next_channel`).

`get hotkeys` value:

```json
{"active": true,
 "bindings": [{"action": "restart", "key": "up", "mods": ["ctrl", "alt"], "combo": "Ctrl+Alt+Up", "registered": true, "error": null, "altgr": null},
              {"action": "mute", "key": "m", "mods": ["ctrl", "alt"], "combo": "Ctrl+Alt+M", "registered": true, "error": null, "altgr": "µ"},
              {"action": "pause", "key": null, "mods": [], "combo": null, "registered": false, "error": null, "altgr": null}],
 "keys": ["0", "1", "...", "up"], "mods": ["alt", "cmd", "control", "ctrl", "shift", "win"], "problems": []}
```

`active`: the hotkeys are registered now (the extension is armed). `registered: false` with `error: "already_owned"` means another program owns that chord. `problems` lists entries of `keymap.json` that were skipped: an unknown key or modifier never disables the other hotkeys.

### Settings page

`GET /settings?token=<token>` on the HTTP port (the `settings_url`) serves the settings page, on this API: Speech (the voice engine's status from `runtime.engine_status`, voice with previews, rate, mute level, verbosity, background sessions), Summary (mode Off, Tidy, Natural or Brief, the instruction of each style, the model, live reading: `read_mode` Immediately, Queue with its `minqueue` stepper shown only in Queue, or When done, gated while a summary mode is on), Audio (speech volume, other apps, duck level, the folder for your own chimes and which ones are in use), Sessions (`channel_prefs`: name and audio per session; switch announcements), Hotkeys (capture, unbind, reset, AltGr and ownership warnings; Flush skips: This session or Everything queued, the `flush_scope`), Advanced (summary timeout and settle time) and System (version, uptime, protocol, the settings file). Since #214 the page has no engine picker: Kokoro is the engine and OneCore its automatic fallback. The `engine` key stays for hosts as an advanced setting; when it names another engine, the page offers a "Use Kokoro" button. The per-session `voice` of `channel_prefs` is stored but not applied, so the page does not show it. The agent sections say so while no client enabled `agent`. It uses only the HTTP API above, with the token filled in by the runtime. It is answered only while `system` is enabled (`404` before), only with the token (`401`) and only for the `Host` `127.0.0.1:<http_port>` or `localhost:<http_port>` (`403`, against DNS rebinding). It is sent with `Content-Security-Policy` (`frame-ancestors 'none'`, connections to itself only), `Referrer-Policy: no-referrer` and `Cache-Control: no-store`. The API sends no CORS headers, so other origins cannot read its replies.

```json
> {"type": "hello", "token": "...", "extensions": ["agent", "system"]}
< {"ok": true, "extensions": ["channels", "agent", "system"], ...}
> {"type": "set", "key": "audio_mode", "value": "duck"}
< {"ok": true, "key": "audio_mode", "value": "duck"}
> {"type": "set", "key": "hotkeys", "value": {"action": "mute", "key": "m", "mods": ["win", "alt"]}}
< {"ok": true, "key": "hotkeys", "value": {"active": true, "bindings": [...], ...}}
> {"type": "get", "key": "settings_url"}
< {"ok": true, "key": "settings_url", "value": "http://127.0.0.1:50312/settings?token=..."}
```
