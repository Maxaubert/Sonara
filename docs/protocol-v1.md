# Sonara protocol v1

The contract between `sonarad.exe` (the Sonara runtime) and its clients: apps that bundle Sonara,
SDKs (`@sonara/client`, `sonara-client`) and anything else on the same PC. This document covers the
**core** (protocol 1.5), which every runtime offers, [external engines](#external-engines)
(capability `engines`), and the [`channels`](#extension-channels), [`agent`](#extension-agent) and
[`system`](#extension-system) extensions.

Source of truth in code: `crates/sonarad` (server), `crates/sonara-reader` (the reader behind it),
`crates/sonara-channels` (the `channels` extension), `crates/sonara-agent` (`agent`),
`crates/sonara-system` (`system`). Black-box tests: `conformance/`
(`python -m pytest conformance -q` after `cargo build -p sonarad`). Spec:
`docs/plans/2026-10-02-sonara-runtime-spec.md` sections 3 and 4.

Since 0.11 (#202) the Claude Code plugin runs this runtime too. The retired Python daemon's older
protocol was removed with its package (#248); it stays in the git history before 0.21.0.

## Contents

- [Discovery](#discovery)
- [Transport](#transport)
  - [TCP JSON lines (`port`)](#tcp-json-lines-port)
  - [HTTP (`http_port`)](#http-http_port)
- [Requests and replies](#requests-and-replies)
  - [`hello`](#hello)
  - [`speak`](#speak)
  - [`control`](#control)
  - [`set` / `get`](#set--get)
  - [`voices`](#voices)
  - [`subscribe` (TCP)](#subscribe-tcp)
- [Events](#events)
- [Errors](#errors)
- [Lifetime](#lifetime)
- [Takeover](#takeover)
- [Versioning](#versioning)
- [Saved settings](#saved-settings)
- [Testing aids](#testing-aids)
- [Examples](#examples)
- [External engines](#external-engines)
  - Full contract: [protocol-v1-engines.md](protocol-v1-engines.md)
- [Extension `channels`](#extension-channels)
  - [Messages](#messages)
  - [Setting](#setting)
  - [State](#state)
- [Extension `agent`](#extension-agent)
  - [Messages](#messages-1)
  - [Settings](#settings)
  - [Events](#events-1)
- [Extension `system`](#extension-system)
  - [Settings](#settings-1)
  - [Shutdown](#shutdown)
  - [Voice previews](#voice-previews)
  - [Spoken cues](#spoken-cues)
  - [Hotkeys](#hotkeys)
  - [Settings page](#settings-page)

## Discovery

**Home folder:** `%LOCALAPPDATA%\Sonara`, or `SONARA_HOME` if set, or `sonarad --home <dir>` (the
flag wins).

**`runtime.json`** in the home describes the running instance. It is written atomically (a temp
file, then a rename) with an ACL that lets only the current user in, and removed when the runtime
exits cleanly.

```json
{
  "pid": 12345,
  "port": 50311,
  "http_port": 50312,
  "token": "64 hex characters",
  "version": "0.9.7",
  "protocol": {"major": 1, "minor": 4},
  "capabilities": ["core", "speak", "control", "set", "get", "voices", "subscribe", "events.state", "events.items", "events.log", "engine_status", "engines"],
  "extensions": ["channels", "agent", "system"],
  "started_at": "2026-10-02T10:40:45Z"
}
```

**Connecting** (what every SDK's `connect()` does):

1. Read `runtime.json`. If the file exists and its `pid` is alive, connect to `port` and send
   `hello`.
2. Use the instance if `protocol.major` is 1 and its `capabilities` (and `extensions`) cover
   everything the client requires.
3. Otherwise start the bundled runtime: `sonarad.exe --home <home>` (no console window:
   `CREATE_NO_WINDOW`), wait up to 5 s for a `runtime.json` whose `pid` is the new process, then
   `hello`.
4. If the running instance is incompatible (another major, a missing capability), send `hello` with
   `takeover: true`. See [Takeover](#takeover).

A stale `runtime.json` (the pid is gone after a crash) is overwritten by the next runtime.

**One instance** per user: the runtime holds the named mutex `Local\Sonara-Runtime-<hash>`, where
`<hash>` is the FNV-1a 64-bit hash (16 hex digits) of the user's SID string. For a home other than
the default `%LOCALAPPDATA%\Sonara`, the hash covers the SID, a newline and the home's canonical
path in lower case, so separate homes (tests, a portable bundle) are separate instances. A second
`sonarad` for the same user and home prints `another instance is already running ...` and exits with
code 3.

## Transport

Both transports bind `127.0.0.1` on ephemeral ports and never another address. Every request is a
JSON object with a `type`; requests are applied in the order they arrive on a connection.

### TCP JSON lines (`port`)

- One UTF-8 JSON object per line (`\n`), both ways; at most 1 MiB per line.
- The **first message must be `hello` with the token**. Anything else (another type, a wrong token,
  invalid JSON) is answered with `E_AUTH` and the connection closes. So is a connection that sends
  no successful `hello` within 5 s. A `hello` that fails for another reason (`E_UNSUPPORTED`,
  `E_INCOMPATIBLE`, `E_BUSY`) leaves the connection open and unauthenticated, so the client may send
  `hello` again.
- Replies and events share the connection: a reply has `ok`, an event has `event`. Replies come in
  request order.
- Invalid JSON after `hello` is `E_BAD_REQUEST`; the connection stays open.

### HTTP (`http_port`)

- `POST /v1/<type>` with header `Authorization: Bearer <token>`. The body is the request without
  `type` (the path gives it), a JSON object; an empty body is `{}`. At most 1 MiB.
- No `hello` is needed (the bearer token authenticates each request), but `POST /v1/hello` works,
  including `keep_alive` and `takeover`.
- The reply body is the same JSON as on TCP. Status: 200 for `ok: true`; for errors `E_AUTH` 401,
  `E_UNKNOWN_TYPE` and `E_NOT_FOUND` 404, `E_BUSY` and `E_INCOMPATIBLE` 409, `E_ENGINE` 500,
  `E_FORBIDDEN` 403, anything else 400.
- `GET /v1/events?events=state,items,log` (default: all three) is a Server-Sent Events stream. Each
  event is `event: <name>` plus `data: <the same JSON as on TCP>`; a `: ping` comment comes every 15
  s. `subscribe` over POST is `E_BAD_REQUEST`.

## Requests and replies

Every request may carry `id` (any JSON value); the reply echoes it. Replies are
`{id?, ok: true, ...}` or `{id?, ok: false, error: {code, message}}`. **Unknown fields are ignored**
in every request, and clients must ignore fields they do not know in replies and events.

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

The reply's `extensions` lists the extensions enabled on this runtime now. An extension is enabled
for the whole runtime as soon as any client asks for it (in `extensions` or `require`) and stays
enabled until the runtime exits; until then its messages, actions and keys are `E_UNSUPPORTED`.
`runtime.json` lists in `extensions` the ones this runtime offers.

**Capabilities** of protocol 1.0: `core`, `speak`, `control`, `set`, `get`, `voices`, `subscribe`,
`events.state`, `events.items`, `events.log`. Protocol 1.1 adds `engine_status` (readiness and model
download progress in `state.engine_status`). A later minor adds capability strings for what it adds,
so a client can `require` them.

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

`{"type": "set", "key": "<key>", "value": <value>}` and `{"type": "get", "key": "<key>"}`; both
reply `{ok: true, key, value}` with the value now in force.

| key | value |
|---|---|
| `volume` | integer 0..=100 (percent) |
| `rate` | integer 100..=400 (words per minute) |
| `voice` | a voice `id` or `name` of the current engine; `null` for the engine default. `get` returns the id or `null` |
| `engine` | an engine id: `kokoro` or `onecore` (`fake` in test runs), or the id of an [external engine](#external-engines) the user added. Switching resets a voice the new engine lacks |
| `debug_log` | `true` or `false` (default `true`, runtime 0.13.2, #219): the troubleshooting log records text (what was read, the messages received, the hooks' raw input); `false` keeps every text out of `logs\` (see [Saved settings](#saved-settings)). A host key: it needs no extension |

A rate, voice or engine change applies to chunks synthesized from then on. Out-of-range or wrongly
typed values are `E_BAD_REQUEST`; an unknown voice or engine is `E_NOT_FOUND`; an unknown key is
`E_BAD_REQUEST` (an extension's key, such as `audio_mode`, is `E_UNSUPPORTED`).

### `voices`

`{"type": "voices", "engine?": "<id>", "refresh?": false}` replies `{ok: true, voices: [...]}` for
one engine or all:

```json
{"id": "...", "name": "Microsoft Zira", "language": "en-US", "engine": "onecore", "license_class": "os", "installed": true}
```

`license_class` is `permissive`, `os` or `external` (protocol 1.2, the voices of an [external
engine](#external-engines)). `installed: false` means listed but not yet able to speak (voice data
missing, a model still to download). An unknown engine is `E_NOT_FOUND`.

**An unsaved profile** (protocol 1.4, runtime 0.18.0, #227, capability `engines`):
`{"type": "voices", "profile": {...}, "secret?": "<key>"}` lists the voices of a profile that is not
saved, so a client (the settings page) can offer the voices before the user picks one. `profile` is
a [profile](protocol-v1-engines.md#profile) whose `id` and `voice` may be missing (any `id` sent is
ignored); it is built for this request only, with `secret` as its only key (bound to the profile's
origin, as in `engine_add`; `key_ref` `env:NAME` reads the variable under the same rules), never
with another profile's stored key. Nothing is saved, stored, registered or logged (`voices` gets no
`in` line, and `secret` is never written). The reply is as for a saved engine: the fetched voices,
or none plus `error: {reason, message}` when the fetch fails. A `command` profile is `E_FORBIDDEN`
(a request never names a program to run), a profile that is not an object or fails validation is
`E_BAD_REQUEST`, and a runtime without external engines answers `E_UNSUPPORTED`.

For an external engine (`engine` naming one), the runtime asks its provider for the list when its
copy is older than 10 minutes or `refresh` is `true` (the request waits up to 10 s). A failed fetch
still answers `ok` with the voices known (at least the engine's own voice) and an additive
`error: {reason, message}`. Without `engine`, external engines contribute the lists they already
have, never a network request. While Sonara is [muted](#external-engines) no list is fetched: the
voices known, with `error: {reason: "muted", message}` when a fetch was due (`refresh`, an old copy,
or an unsaved `profile`, which then lists only its own voice). An external engine also speaks voice
ids it does not list (a cloned voice, a provider's voice id), so `set voice` accepts any id while
one is current.

**Engines.** `onecore` is Windows' own speech: zero download, licence class `os`. `kokoro` is
Kokoro-82M v1.0 (Apache-2.0 weights) on Microsoft's ONNX Runtime with GPL-free phonemes, licence
class `permissive`, 28 English voices (`af_heart`, the default, `af_sarah`, `bm_george`, ...; ids
also accept the `kokoro:` prefix and display names such as `Heart (Kokoro)`). Its model (about 354
MB) is downloaded on first use into `<home>\models\kokoro\v1.0\` from pinned URLs with pinned
SHA-256 values, resumed after an interruption (a download in progress is `<file>.part`), and checked
before use (`verified.json` there remembers checked files by size and time); a host may pre-seed
that folder with the two files (`kokoro-v1.0.onnx`, `voices-v1.0.bin`). Until Kokoro is ready
(downloading, a failed download waiting to retry, no ONNX Runtime) it speaks with `onecore` at once,
and `state.engine_status` says so. Where `onecore` cannot speak (its warm-up fails or it lists no
voices), there is no fallback: `engine_status` names none, and each item waits for Kokoro while the
model downloads or loads (up to 5 minutes), so no speech is dropped. The runtime does not idle out
while the model downloads or loads. A failed download is retried after 30 s, then after twice as
long each time up to 30 minutes, never on every sentence. The rate maps to Kokoro's speed as
`rate / 200`, from 0.5 to 2.0.

### `subscribe` (TCP)

`{"type": "subscribe", "events": ["state", "items", "log"]}` (omitted: all three) replies
`{ok: true, events: [...]}` and from then on sends those events on this connection. Subscribing
again replaces the set (`[]` stops events). An unknown stream name is `E_UNSUPPORTED`; an
extension's stream (`earcons` of `agent`, `cues` of `system`) is asked for by name and is
`E_UNSUPPORTED` while the extension is off. When `state` is included, the first event is the current
state.

A client that does not read its events never slows the reader: past 256 unread events, events are
dropped for that client. Each `state` event is a full snapshot, so the next one brings a player up
to date.

## Events

```json
{"event": "state", "seq": 12, "now_playing": {"item_id": 7, "label": "build", "text": "Build finished.", "chunk": 0, "chunks": 2}, "queued": 0, "paused": false, "muted": false, "volume": 100, "rate": 200, "voice": null, "engine_status": {"engine": "kokoro", "ready": true, "status": "ready"}}
{"event": "item", "item_id": 7, "phase": "started"}
{"event": "log", "message": "synthesis failed: ..."}
```

- `state` (stream `state`): sent on change only, `seq` strictly increasing. `now_playing` is `null`
  when idle; its `text` is the chunk being read. `queued` counts items after the current one.
  `engine_status` (below) says whether the current engine speaks with its own voice yet.
- `item` (stream `items`): `phase` is `started`, `finished`, `skipped` or `failed`. An item ends
  `failed` only when none of its chunks could be played; a failed chunk is skipped and logged.
- `log` (stream `log`): a line worth showing in a log, such as a failed synthesis or an engine that
  is not ready. A change of the engine's readiness is logged too
  (`engine 'kokoro' is downloading its model; speaking with onecore meanwhile`,
  `engine 'kokoro' is ready`), not each bit of download progress.

**`engine_status`** (protocol 1.1; a 1.0 runtime sends only `engine`):

| field | meaning |
|---|---|
| `engine` | the current engine id |
| `ready` | `true` when it speaks with its own voice |
| `status` | `ready`, `loading` (its model, a few seconds), `downloading`, `waiting` (the last download or load failed; it retries later) or `unavailable` (it cannot run in this install, for Kokoro: no `onnxruntime.dll`; for OneCore, since runtime 0.21.5 (#274): no usable Windows voice, with `message` saying how to add one, until a sentence was spoken) |
| `progress` | `{done, total}` bytes, while `downloading` |
| `fallback` | the engine speaking meanwhile (`onecore`), while not ready |
| `message` | why it is not ready, after a failure |
| `reason` | protocol 1.2, external engines: why it is not speaking itself, one of `no_key`, `auth`, `quota`, `rate_limited`, `network`, `timeout`, `server`, `bad_voice`, `bad_config`, `format` (see [External engines](#external-engines)) |

```json
"engine_status": {"engine": "kokoro", "ready": false, "status": "downloading", "progress": {"done": 104873984, "total": 353746785}, "fallback": "onecore"}
```

A change of `engine_status` alone (download progress, about four times a second at most) is a new
`state` event with a new `seq`. The runtime counts these changes once, so every client sees the same
`seq` for the same state, however long it has been subscribed.

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
| `E_FORBIDDEN` | protocol 1.3: a request no client may make over the protocol, from any transport or SDK: `engine_add` of a `command` engine, or one that would replace a `command` engine (see [External engines](#external-engines)) |

An error of an external engine (`engine_test`) carries an additive `reason` (protocol 1.2), the same
values as `engine_status.reason`:
`{"ok": false, "error": {"code": "E_ENGINE", "message": "OpenAI refused the key (401): Incorrect API key provided", "reason": "auth"}}`.

## Lifetime

A client is a TCP connection that completed `hello`, or an open SSE stream; a plain HTTP request
only counts as activity. The runtime exits 30 s (`--idle-exit <seconds>`) after the last client left
and nothing is being read (a paused item does not count), unless a client sent `keep_alive: true` or
it runs with `--standalone`. The countdown starts when `runtime.json` is written, so a slow start
never uses it up before the first client can connect. The exit is decided atomically with the
requests that start speech: a `speak` or `control` either comes first (and keeps the runtime while
it is read) or is answered `E_BUSY`, never accepted and then lost; any request that reaches the
runtime while it exits is `E_BUSY` (#194), also an HTTP request whose connection was still waiting
to be accepted when the exit was decided: the runtime answers it before the process ends (runtime
0.20.4, #247). Ctrl+C and a [`shutdown`](#shutdown) (extension `system`) end it cleanly. On every
clean exit it stops speech and removes `runtime.json`.

## Takeover

When a client finds an instance it cannot use (another protocol major, a missing capability):

1. It sends `hello` with the token and `takeover: true`.
2. If nothing is playing or queued (a paused item counts as busy), the runtime replies
   `{ok: true, takeover: true, ...}` and from then on answers `speak` and `control` from any client
   with `E_BUSY`, so nothing is accepted and then dropped. It closes the connection, stops audio,
   releases the single-instance lock, removes `runtime.json` and exits with code 0. The client waits
   for the process to end (or, at the least, for `runtime.json` to go), then starts its bundled
   runtime.
3. Otherwise the reply is `E_BUSY`; the client retries after the current item (bounded, 30 s in
   total), then gives up with `E_INCOMPATIBLE`.

## Versioning

Semantic versioning on `protocol: {major, minor}`. A minor only adds optional fields, message types,
capabilities and events; it never changes the meaning of what exists. 1.1 (runtime 0.10.0) added the
readiness fields of `engine_status` and the `engine_status` capability. 1.2 (runtime 0.15.0, #224)
added [external engines](#external-engines): the capability `engines` and its five core messages,
`engine_status.reason`, `error.reason`, `voices.refresh` and the licence class `external`. Runtime
0.16.0 (#225) added the kinds `elevenlabs`, `azure` and `google`, runtime 0.17.0 (#226) `cartesia`,
`deepgram` and `command` (new values of `engine_list.kinds`). 1.3 (runtime 0.17.0, #226) added the
message `engine_reload` and the error code `E_FORBIDDEN`: a `command` engine runs a program on the
user's PC, so it is never added or changed over the protocol (the kind is new in the same release,
so no client depended on adding one). 1.4 (runtime 0.18.0, #227) added `voices` with `profile` and
`secret` (the voices of an unsaved profile). 1.5 (runtime 0.19.0, #235) added `engine_models` (a
provider's models, live), the profile's `send_mode` ("Send to the engine") and the profile view's
`takes_model`, `model_required`, `model_list`, `missing` and `send_mode`; the same release stopped
filling in a default model or voice for any external engine (a profile without one it needs is kept
and reports `bad_config`, "choose a model" or "choose a voice"), so the view's `model` and `voice`
are only what the profile sets. Runtime 0.13.0 (#214) narrowed `verbosity` to `everything` and
`skip_code` (old values are accepted as aliases: `all` is `everything`, `medium` and `quiet` are
`skip_code`, so `get` returns the new names) and added `engine_status` to `runtime`; no shipped
client depended on the old verbosity values, so the protocol minor was not bumped for it. Clients
ignore unknown fields and event types. A new major is a new protocol: a client that needs it takes
over an idle older runtime.

## Saved settings

Every setting a client changes with `set` is saved in the home and applies again when the next
runtime starts (#201): the core keys before the runtime accepts its first client (so before the
first speech), an extension's keys when a client enables that extension. Hotkeys that change a
setting (the mute cycle, faster, slower) save it too.

**Defaults** (#202). A setting nobody set has the runtime's default, the Claude plugin's product
settings: `voice` `af_sarah` (with an engine that has it; otherwise the engine's default voice),
`rate` 250, `volume` 100, `channel_announce` `"on"`, `mute_level` 0, `verbosity` `"skip_code"`,
`read_mode` `"done"` (#222), `flush_scope` `"session"` (#228), `minqueue` 5, `background_policy`
`"all"`, summaries off, `audio_mode` `"pause"`, `duck_level` 30, `debug_log` true. The tables below
give the same defaults. `sonarad` applies them to the layers itself; the library crates keep their
own (rate 200, `"everything"`, `"queue"` with 1, `"earcon_only"`, `"off"`).

| file in the home | holds |
|---|---|
| `config.json` | only the keys a client set (even to the default), never the defaults: `engine`, `voice`, `rate`, `volume`, `channel_announce`, `mute_level`, `verbosity`, `read_mode`, `flush_scope`, `minqueue`, `background_policy`, `summaries` (only the fields that were set, plus `prompts`), `audio_mode`, `duck_level`, `debug_log`. A `_migrated` key records the migration below |
| `session_prefs.json` | per channel: `label`, `voice`, `muted` (see `channel_prefs`), the 200 most recently changed |
| `engines.json` | the [external engines](#external-engines) added with `engine_add`: `{"format": 2, "engines": [profile, ...]}`, never a key. An `env:` entry may carry `key_origin` (see Keys below), which only the user writes in the file; a format 1 file is migrated at the first start (its keys and `env:` entries are bound to their addresses then) and saved as format 2. A file that is not JSON is copied to `engines.json.bad` (and logged) and counts as empty until the next change; an entry that fails validation or names a kind this runtime lacks is kept, listed and not used. Read once at start, before the first speech, so a saved `engine` naming one applies at once |
| `keymap.json` | the hotkey overrides (see [Hotkeys](#hotkeys)) |
| `earcons\` | the user's own earcons: `<kind>.wav` replaces that kind's bundled clip (see [Custom earcons](#custom-earcons)); created empty at start, only read |
| `logs\` | every log of the home, under one budget (#219, crate `sonara-log`): each stream (`sonarad.log`, `hook.log`) rotates at about 1 MB into `<stream>.1.log`, `<stream>.2.log`, ... (1 the newest), and everything in the folder together (also files other writers leave there, such as `bootstrap.log`) stays at or under 10 MB: a line that would pass it first deletes the oldest files (by last write), and as a last resort empties the stream's own file (when a viewer holding an older segment stopped its rotation). Writers in several processes (the runtime's threads, each hook call) take an OS lock on `logs\.lock` per line, so no line is torn, lost or duplicated across a rotation; a writer (the runtime or a hook) waits 50 ms for it at most and otherwise skips its line. A line over 256 KB is clipped |
| `logs\sonarad.log` | each line starts with a UTC time with milliseconds (`2026-10-03T10:59:14.123Z`). One line per start (`sonarad <version> started (pid <pid>): engine <id> <status>; home <dir>`), the engine's readiness changes (model downloaded, loaded or failed; not the progress), external engines (`engine add id=<id> kind=<kind> host=<host>[ key=set]`, `engine remove id=<id>`, `engine key id=<id> set\|cleared`, never the key; `engine <id> fallback reason=<reason>[ status=<http>] -> <fallback>: <message>` when the built-in voice read instead, at most one per engine and reason per minute, and `engine <id> recovered`, never the text), the migration, saved values that could not be applied, custom earcons used or refused. Activity (#217), lines that never carry text: `read start item=<id> session=<label, else channel id, else direct> chunks=<n>` (` kind=announce` for a session switch announcement), `read end item=<id> finished\|skipped\|failed`, `reader paused` / `reader resumed` (any source: hotkey, CLI, settings page, SDK), `ask kind=<kind> session=<label or channel>`, `hotkey <action>[ <detail>]` (`hotkey mute level=2`, `hotkey flush session=<label>\|announcement session=<label>\|direct\|idle[ scope=session\|all][ others=<labels>]` (`scope` with `agent`; `others`: the sessions whose ready messages scope `all` dropped, comma separated), `hotkey next_channel session=<label>`, `hotkey faster rate=275`, `... failed: <error>`), and from the `system` extension `media pause apps=<names> (reason: reading item=<id>[ session=<label>])`, `media resume apps=<names> (reason: idle\|paused or muted\|disarmed\|mode <mode>\|shutdown)`, `duck apps=<names> level=<n> (reason: ...)`, `restore apps=<names> (reason: ...)`, failures (`media pause failed ...`, `restore failed apps=...`) and `startup sweep: restore\|media resume[ failed] apps=<names>`. Only engages and restores that touched an app are logged. A value with a space is quoted. Troubleshooting (#219), lines that carry text only while `debug_log` is on: `in {json}` for every message received from an authenticated connection (nothing a connection sends before its `hello` is logged, and its refusals at most one line per 10 s) but `get`, `voices` and HTTP `hello` (compact, without the token, credential-looking values `[redacted]`, `options` as their labels, a string over 4 KB clipped; `in failed type=<t> E_<CODE>: <message>` when refused); `agent <message> channel=<id> speak kind=<prose\|question\|permission\|plan\|tool\|summary> entry=<n>[ decision][ waits=<why>] text=...` for text the agent added, `agent <message> channel=<id> store kind=<k> entry=<n>[ decision] muted=<1\|2> text=...` for text stored while muted (#243: read by a switch or Up, never on its own), `agent <message> channel=<id> <kind>: <why it was not spoken>` (`summary: not made: mute level <n>, the prose is stored as it is`, `skip_code`, held by `read_mode` (`held: <n> chunk(s) wait for minqueue <m> or the turn end`, `held: waits for the turn end (read_mode done), <n> chunk(s)`, `held: <n> chunk(s) wait for the end of the paragraph (send mode message)`; `dropped: <n> held chunk(s) (answered\|turn_start\|stop\|flush, read_mode <mode>)` when an answer, a new turn, a stop or a flush drops what was still held; with summaries on, `prose: dropped: <n> chunk(s) kept for the summary (<reason>)`, `summary: cancelled (<reason>): <n> summary in flight, <n> summary waiting for the earlier ones, the settle window` and `<question\|permission\|plan>: dropped: waited for the summary (<reason>)` when an answer or a stop drops them, `<question\|permission\|plan>: spoken now: the summary it waited for was flushed` when a flush releases them, `<prose\|code>: dropped: flushed reply` and `tool: not announced: flushed reply` for the rest of a flushed reply (#228)), the permission prompt of an unanswered question, summaries), `agent <message> channel=<id> dropped: late text ...` (stamped before the last `turn_start`), `agent <message> earcon <kind>`, `agent <message>[ channel=<id>] wipe reason=<turn_start\|answered\|mute\|stop\|flush>`; `drop channel=<id> entry=<n>[ item=<id> (cut while read)][ kind=<k> from=<message>] reason=<why> text=...` for text dropped before it was heard (`replaced by newer text (policy latest)`, `turn_start`, `answered`, `mute`, `stop`, `flush`, `closed`, `muted`); `read text item=<id> session=<s> kind=<k> from=<message> chunks=<read>/<all> text=...` before each `read end`, the exact cleaned text that went to the voice (`kind` also `announce` or `speak`); `read drop item=<id> session=<s> kind=<k> from=<message> unread` for an item the reader dropped unread; `cue text=...` for a spoken cue. Text fields are JSON strings; with `debug_log` false they are left out (and an `in` line keeps only the fixed fields: type, channel, kind, t, index, final, key, a `set` value that is a number, a switch or one of the runtime's own choice settings, ...), so no session text is written |
| `logs\hook.log` | one line per `sonara-hook.exe` call: `hook <Event> pid=<pid> <sent\|started the runtime, sent\|dropped (no runtime answered)\|nothing to send> ms=<n>[ session=<id>][ tool=<name>][ notification=<type>] sent=[messages] payload={raw stdin}` (a string field over 4 KB clipped, credential-looking values `[redacted]`, the `tool_input` of a tool other than `AskUserQuestion` reduced to `{"fields":[names]}` and any `tool_response` omitted, `raw="..."` when stdin is not JSON). With `debug_log` false in `config.json`: `sent=[types]` and no payload |

Files are written atomically (a temp file, then a rename). A `config.json` that is not a JSON object
gives the defaults and is copied to `config.json.bad` (and logged) before the next save replaces it.
A value out of range is not applied (and logged), the others still apply; it stays in the file, like
a key this runtime does not know (from a newer release), until a client sets that key. A saved value
the reader refuses at start, such as a voice the current engine lacks (a Kokoro voice from the
Python plugin while only OneCore is installed), is logged and kept in `config.json`, so it applies
once it is available; the default is used meanwhile. `--engine` on the command line wins over a
saved `engine` (except that `--engine fake` keeps a saved external engine, with the fake engine as
its fallback), which wins over the default choice below; a saved `engine` may name an external
engine; a saved engine that cannot start is logged and the default choice is used. Setting `engine`
to another engine replaces a saved voice that engine lacks with the voice in force; setting the same
engine again keeps it.

**Migration from the Python plugin.** The first runtime on the default home
(`%LOCALAPPDATA%\Sonara`) that has no `config.json` imports the plugin's settings from
`%USERPROFILE%\.sonara` (another folder, or another home: `--migrate-from <dir>`): its `config.json`
(voice, rate, speech volume, audio mode, duck level, mute level, verbosity, minimum queue,
background policy (`earcon_only`, or any other value as `all`), summary mode, command, model,
timeout, settle time, style and custom prompts), `keymap.json` (`nav_start` becomes `restart`,
`next_session` becomes `next_channel`; only when the home has no `keymap.json`) and
`session_prefs.json` (the session `name` becomes `label`; only when the home has none), and the
user's own earcons (`config.json` `earcons: {kind: path}`; each file that exists is copied to
`earcons\<kind>.wav`, unless one is there; paths to the plugin's bundled WAVs are skipped). Every
value of a format 2 file (the plugin saved only what the user set) is saved, also one equal to the
runtime's default, so a user who chose the plugin's old default keeps it; in a file from before
format 2 (every key written) a value equal to the plugin's old default (rate 200, volume 100,
`audio_mode` `"off"`, `mute_level` 0, `verbosity` `"everything"`, `minqueue` 1, `background_policy`
`"earcon_only"`) counts as unset. Summary fields equal to the runtime's default are not saved; a
Chatterbox voice speaks as `af_heart`, `audio_control: true` is `audio_mode: duck`, a speech volume
above 100 is 100, and in a file from before the plugin's format 2 the old defaults `duck_level: 20`
and `summary_timeout: 20` count as unset. The cue voice and fast cues are not imported (the runtime
speaks its control cues in the voice in force). The plugin's folder is only read. The migration
writes `config.json` with the `_migrated` marker, so it runs once; what it did is in
`logs\sonarad.log`.

## Testing aids

`sonarad --engine kokoro|onecore` picks the engine to start with; without it, the saved `engine`,
else `kokoro` when `onnxruntime.dll` is next to `sonarad.exe` (`SONARA_ORT_DYLIB` names another
copy, a development aid), else `onecore`. A Kokoro start verifies or downloads and loads the model
in the background right away. `sonarad --engine fake` uses a deterministic tone engine (10 ms of
audio per character at rate 200; text containing `[fail]` fails to synthesize) and, unless
`--output device` is given, `--output null`: a silent output that keeps real time.
`sonarad --output wav:<dir>` (runtime 0.21.5, #274) is that null output, and it also writes the
PCM of every chunk it is handed to a 16-bit WAV file in `<dir>` (created if needed):
`<seq>-item<item_id>-chunk<n>.wav` per chunk played (numbered in play order; a chunk played again
gets a new file, a streamed chunk grows until it finished), `<seq>-clip.wav` per clip (an earcon, a
preview, `engine_test` with `play`). The files hold the audio as the engine made it (before volume
and mute), all of it even when a stop cut the chunk, so an end-to-end test can check that a real
engine spoke (`tests/embed`). It works with every engine.
`sonarad --system fake` replaces the Windows side of the [`system`](#extension-system) extension
with fake apps, media sessions, hotkeys and keyboard layout kept in `<home>\fake-system.json` (read
and written on every operation), so a test can set up the "apps", kill the runtime and check what
the next one restores. `sonarad --keys fake` keeps external engine keys in `<home>\fake-keys.json`
instead of Windows Credential Manager (`--keys windows`, the default), so tests never touch the
user's credentials. They are meant for tests and the conformance suite, not for apps. With
`--engine fake` there is no Kokoro engine, so nothing is ever downloaded.

`sonarad --no-external-engines` refuses [external engines](#external-engines): no `engines`
capability, every `engine_*` message is `E_UNSUPPORTED`, and `engines.json` is not read. A host that
bundles Sonara and must not send text off the PC starts it this way.

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

Protocol 1.2, capability `engines`: speech engines the user adds at run time (cloud APIs, local
servers, a program on this PC), the `engine_*` messages and the voice rule. The full contract is
[protocol-v1-engines.md](protocol-v1-engines.md).

## Extension `channels`

Spec section 4.2, L2 (`crates/sonara-channels`). Several named sources (terminal tabs, chats) share
the one reader: each **channel** keeps its own messages and policy, and one channel is read at a
time. Enable it with `hello` `extensions: ["channels"]`. Black-box tests: `conformance/channels/`.

**Model.** A channel holds its current **batch**: the messages sent to it since it was last caught
up, with a read position. Messages wait in their channel and go to the reader one at a time, only
when the reader is idle, so text spoken without a `channel` (core `speak`) is read first. Heard
messages stay, so a manual return can replay the batch; a message sent to a channel that is caught
up (and not still reading its last message) starts a new batch.

- **Policy** `latest` (the default): a new message replaces the channel's unread messages, so the
  newest one is always read and never dropped ("one message, always the last"). `queue`: every
  message is read, in order.
- **Who reads next:** a replay the user started (`restart`, or `next_channel` landing on a replay)
  keeps the floor until the messages its batch had when it started are read (runtime 0.21.4, #271;
  text the session writes during the replay is read as live reading): no other channel's message
  and no decision (`ask` with `agent`) cuts in at a message boundary; what arrived meanwhile is read
  after it, decisions first. `flush`, `control stop` of that channel or without a channel (also a
  `turn_start` or an `answered` of its session), `next_channel`, `speak` with `interrupt` into
  another channel and muting it end that hold. Else a
  decision is read first (after the item playing); then a channel whose batch a decision cut, which
  resumes where it stopped; then the channel being read keeps the floor until its batch is read;
  then the focused channel; then the first channel (in opening order) with something unread. A
  channel you left with `next_channel` is not resumed on its own until it gets a new message.
- **Muted channels** (`channel_prefs` `muted`): a muted channel's messages wait, unread, and it
  never takes the floor (muting the channel being read cuts its item); `next_channel` skips it
  unless every channel is muted. Unmuted, its waiting messages are read.
- **Agent batches** (with `agent`, runtime 0.20.2, #243): the batch the agent writes into a
  session's channel is the session's latest message. It grows while the message goes on, also after
  the channel caught up, until the session starts a new turn, an answer comes or the session is
  flushed (`control stop` or `flush` with that channel); `control stop` without a channel and muting
  keep it. Text the agent stores while muted (`mute_level`) joins it as heard (unread when a replay
  of the channel is in progress, so the replay reads it). An answered decision leaves the batch, so
  a replay reads the latest message without it: `answered` and `turn_start` of the session take out
  every question, permission and plan item (unread ones are logged `drop ... reason=answered`); a
  `tool` only those already read aloud (a parallel tool or a subagent's tool shares the session, so
  an unread decision or one stored while muted stays); `turn_end` every one read or stored, never an
  unread one. A decision not answered yet stays and is read by a replay. A turn that comes without
  `turn_start` (a background task or a subagent woke the agent) starts a new batch at its first
  `tool` or `ask` after the `turn_end`; prose streamed before that still joins the previous batch.
- **Announcements:** a switch to another channel is announced by a short item before its first
  message: `"<label>."`, or `"<label>, reading again."` when the batch is replayed from the top. An
  automatic hand-off is announced when the channel differs from the one that read last (never for
  the first channel to read), also when that channel has closed since and when the new channel takes
  the floor with priority (a decision) or with `interrupt` (runtime 0.20.1, #241), but not for a
  channel with the same `host_tab` as the closed one that read last (a new session in the same host
  tab, after `/clear` or a relaunch, replaces it; without a `host_tab` such a new session is
  announced); `next_channel` is always announced. Without `agent`, a channel without a `label` is
  not announced. `set channel_announce "off"` turns announcements off. With the `agent` extension on
  (an agent host such as the Claude plugin) the texts are the Python plugin's,
  `"Session changed: <label>."` and `"Session changed: <label>, reading again."` (a channel without
  a label: `"Session changed."` and `"Session changed, reading again."`, so no switch is silent,
  #241), and every announcement is preceded by the `session_change` earcon, for automatic hand-offs
  and manual switches alike (#209); not at `mute_level` 2. The chime waits for any earcon playing,
  and the announcement waits until the chime ended, so the user hears the earcons, then the label
  (#238). The announcement waits for every earcon queued, also when its own chime was dropped as a
  duplicate or because the queue was full. Without `agent` the texts stay generic.

### Messages

| type | fields | effect |
|---|---|---|
| `channel_open` | `channel`, `label?`, `host_tab?`, `policy?: latest\|queue`, `keep_label?` | open a channel, or update an open one (its messages stay; `policy` omitted keeps the current one; a `label` replaces the one it has). With `keep_label: true` (runtime 0.20.3, #245) a channel that already has a label keeps it (and the page's `client_label` stays the first one); a channel without one takes `label`. The user's `channel_prefs` label wins either way. Reply `{channel, created, policy}` |
| `channel_close` | `channel` | close it and forget its messages; if it is being read, its item is cut and the next channel follows (announced: the user heard the closed one last, #241) |
| `focus` | `channel` | read this channel next once the channel being read has finished its batch (does not cut) |
| `speak` | `channel?` plus the core fields | with `channel`: add a message to it (opened with the defaults if needed). `mode` overrides the policy for this message (`replace` drops the channel's unread messages, `append` keeps them). `interrupt: true` reads it now: it goes before the channel's unread messages, the current item is cut and the switch is announced; another channel's message cut this way is read again once this channel's batch is read. Reply `{item_id, channel, dropped}`: `item_id` is the reader item when the message went to the reader at once, `null` while it waits in its channel (behind other messages or an announcement); `dropped` counts the unread messages it replaced |
| `control` | `channel?` plus the core `action`, or `action: next_channel` or `flush` | see below |

`channel` is a non-empty string (`E_BAD_REQUEST` otherwise); an unknown channel in `channel_close`,
`focus` or `control` is `E_NOT_FOUND`.

**`control` once `channels` is enabled.** Without `channel` the actions are the core ones, except:

- `stop` also skips every channel to its end (nothing more is read until a new message; heard
  messages stay replayable).
- `restart` while nothing is playing replays the batch of the channel being read or read last (the
  Claude plugin's Up key), not only its last item; before any channel was read (everything came
  while muted), the focused channel's, else the one written to last (runtime 0.20.2, #243). When
  that channel is the agent's and its batch is empty (its only item was an answered decision),
  `restart` reads nothing.

With `channel`: `stop` skips that channel to its end and cuts its item if it is being read;
`restart` goes back to the start of its item if that channel is being read, else replays the
channel's batch from the top and switches to it (cutting the current item, announced); any other
action applies only while that channel is being read, and is a no-op otherwise.

`next_channel` (reply `{channel}`, `null` when no channel is open) switches now: it moves around the
channels in opening order, skipping channels with nothing to hear (unless all are empty), starting
from the channel being read or the one that read last. It cuts the current item and announces the
target. A fully heard target, landing on the same channel, or returning to a replay in progress
replays the batch from the top; unread messages resume where they stopped (a message cut by the
switch is read again).

`flush` (#228, the flush hotkey; reply `{flushed, channel, scope, others}`) stops what is being read
now. When a channel's item is playing or paused, that channel is skipped to its end and its item
cut, as `stop` with that `channel` (`flushed: "channel"`, `channel` its id); a paused reader is
un-paused. In scope `session` the other channels keep everything and are read next as usual. When a
switch announcement is playing, only the announcement is skipped and the channel it names is read at
once with all its messages (`flushed: "announcement"`, `channel` the announced one): the press was
aimed at the channel that had just ended. Once that channel's first item has started, a flush
flushes it. When the reader is reading text spoken without a `channel`, only that item is skipped
(`flushed: "direct"`). When nothing is being read, `flushed` is `"nothing"`: in scope `session`
nothing changes; with `agent` and scope `all` the other sessions' ready text is still dropped and
listed in `others`. It takes no `channel` (`E_BAD_REQUEST`; use `stop` with the channel). Without
the extension it is `E_UNSUPPORTED`. `scope` is the `flush_scope` in force (`"session"` without
`agent`) and `others` the channels whose ready messages scope `all` dropped too (`[]` in scope
`session`). With `agent` it also drops that session's agent state and skips the rest of its reply,
and scope `all` drops the other sessions' ready messages, see below.

A switch (`next_channel`, `restart` with a channel, `speak` with `interrupt`) only cuts the current
item: text spoken without a `channel` that is already waiting in the reader still plays first, so it
comes between the announcement and the channel's message. A channel item ended from outside the
extension (a core `speak` with `interrupt`, or `skip`) counts as heard: the extension cannot tell it
apart from a user skip, so that message is not read again on its own (`restart` with the channel
replays it).

### Setting

| key | value |
|---|---|
| `channel_announce` | `"on"` (default) or `"off"`: switch announcements |
| `channel_prefs` | the user's preferences per channel, for a settings page. `get`: a list of `{channel, open, reading, client_label, host_tab, label, voice, muted}`, the open channels first (in opening order), then channels with saved preferences only (most recent first). `set {channel, label?, voice?, muted?}` changes the fields given (`null` or `""` clears a label or voice) and replies with the list. A `label` replaces the one the client sends in `channel_open` (also for a channel opened by its first text), so switch announcements say it; `client_label` is the client's. `muted: true` mutes the channel at once and in every later run (see Muted channels, #196); `voice` is saved for the page and not applied yet. `set {channel, forget: true}` forgets a channel that is not the focused one (`E_BAD_REQUEST`): its preferences, and its messages and turn (it is closed if open; a session that died without ending) |

### State

`state.now_playing` gains `channel` and `host_tab` (both `null` for text spoken without a channel;
an announcement belongs to the channel it announces), and `queued` also counts the channels' unread
messages. A `state` event is sent when the reader's state changes, so `queued` catches up with a new
channel message at the next change.

```json
{"event": "state", "seq": 31, "now_playing": {"item_id": 12, "label": "Build tab", "text": "Build finished.", "chunk": 0, "chunks": 1, "channel": "tab-3", "host_tab": "3"}, "queued": 1, "paused": false, "muted": false, "volume": 100, "rate": 200, "voice": null, "engine_status": {"engine": "onecore", "ready": true, "status": "ready"}}
```

## Extension `agent`

Spec section 4.3, L3 (`crates/sonara-agent`). Speech for coding agents and chat assistants on top of
[`channels`](#extension-channels): one channel per agent session, with streamed text, turns,
decisions spoken with priority, earcons, three mute levels and optional summaries. Enable it with
`hello` `extensions: ["agent"]`; it needs `channels`, which is enabled with it (the reply lists
both). Black-box tests: `conformance/agent/`.

**Model.** Each channel has a current **turn**. The agent's text is streamed into it (`stream`),
split into sentences and added to the channel's batch as it completes, whatever the channel's policy
(a turn is many messages). A new turn (`turn_start`) drops what is left of the previous one: its
unread sentences, and its item if it is being read ("one message, always the last"). Text that
arrives late from an earlier turn is dropped (see `t`). Decisions (`ask`) are read before the other
channels as soon as the item playing ends (the batch reading now waits), and play an earcon.

**Background sessions (`background_policy`, #195).** With `"earcon_only"` (the Python plugin's
default; the runtime's is `"all"` since #202) only the focused channel (the session the user
prompted last: the Claude hooks `focus` on every prompt) is read automatically; the other channels
play their earcons, and their text and decisions wait until the user prompts that session (`focus`),
switches to it (`next_channel`) or replays it (`restart`). Exceptions, as in the Python plugin: the
channel focused before keeps the right to finish what it had unread when the focus moved, a summary
(or the raw text standing in for one) is read whatever the focus, and text a host speaks into a
channel (`speak` with `channel`) is always read. With no channel focused nothing is held back.
`"all"` reads every channel in turn.

**Dead sessions.** A channel with no agent message for 6 hours has its turn state freed (as
`channel_close` does for the turn); its channel is closed too when it is neither focused nor being
read and has nothing unread. `channel_prefs` `forget` does it at once.

**Late text (`t` and `turn`).** Senders that run as separate processes (hooks) can deliver the old
turn's last text after the new prompt. Every agent message may carry `t`, the sender's start time in
seconds (any clock, the same one for all senders of a channel, such as Unix time). A `stream`,
`turn_start` or `turn_end` whose `t` is older than the channel's last accepted `turn_start` is
dropped and answered `{stale: true}`; so is one naming, in `turn`, a turn id that a later
`turn_start` replaced. Messages without `t` and `turn` are never stale.

### Messages

| type | fields | effect |
|---|---|---|
| `stream` | `channel`, `delta`, `index?` (default 0), `final?`, `turn?`, `t?`, `label?` | a piece of the agent's text. `index` numbers the deltas of one block (a new block may restart at 0); `final` ends the block and flushes an unfinished sentence. Reply `{stale}` |
| `turn_start` | `channel`, `turn?`, `t?`, `label?` | a new turn: the channel's unread text is dropped and its item cut; a question waiting for an answer and summary work of the old turn are dropped. If the channel is the one being read or read last and the reader is paused, it resumes; **a new turn in another channel keeps the pause on**. Reply `{stale}` |
| `turn_end` | `channel`, `turn?`, `t?`, `label?` | the agent finished: plays `turn_done` and reads text held by `read_mode`. Reply `{stale}` |
| `ask` | `channel`, `kind: question\|permission\|plan`, `text?`, `options?`, `multi_select?`, `notes?`, `hint?`, `hint_once?`, `label?` | a decision, read with priority (after the item playing, before the other channels; a replay the user started is read to its end first, #271). `question`: the text, then `Option n: label.` and its description for each of `options` (strings or `{label, description?}`; an option without a label keeps its number), plays `choice`, and marks the channel as waiting for an answer. `permission`: the pending action, plays `permission`; while a question waits, the permission prompt it fires itself is dropped (no earcon, no text) and clears the mark. `plan`: `"Plan ready. <text>"`, no earcon. `notes` is read after the decision; `hint` too at verbosity `everything`, and `hint_once` after it the first time a channel gets one |
| `tool` | `channel`, `name`, `summary?`, `label?` | the agent runs a tool: clears a waiting question; at verbosity `everything` it reads `summary` (else `"Running <name>."`) after the text held so far (read_mode `done`: the text stays held until the turn ends or a decision) |
| `answered` | `channel`, `label?` | the user answered the question: everything queued for the channel is stale, so its unread text is dropped and its item cut, summary work and held decisions are dropped; the turn goes on |
| `earcon` | `kind` | play an earcon: `choice`, `permission`, `error`, `turn_done`, `nav`, `nav_edge`, `session_change`, `summary_failed` |

`channel` is a non-empty string (`E_BAD_REQUEST` otherwise); a channel is opened with the defaults
when it gets its first text (open it with `channel_open` to give it a label for announcements).
`label` (a string, optional, `E_BAD_REQUEST` when not a string on a message that names a channel,
ignored on `earcon`; runtime 0.20.1, #241) is the channel's label for a channel that has none yet: a
channel the message opens gets it (the user's `channel_prefs` label wins), and an open channel
without a label gets it; a label the channel has is never replaced (use `channel_open` to rename).
The Claude plugin's hook sends the session's project with every message that names the session, so a
session whose first message after a runtime restart is not its prompt is still announced by name.
The project (runtime 0.20.3, #245; before, the last folder of `cwd`, which changed with every `cd`):
in `<repo>\.claude\worktrees\<name>` it is `<repo>`; else the repository of the nearest `.git` at or
above `cwd`, below the user's home folder (`USERPROFILE`; a `.git` folder needs a `HEAD`) (a linked
worktree's `.git` file is followed to its main repository's folder name; a submodule keeps its own);
else `cwd`'s last folder. Its `channel_open` sends `keep_label: true`, so a session keeps the first
name it got. With the extension on, `channel_close` also forgets the channel's turn, and
`control stop` without a channel also drops every channel's summary work and held decisions.
`control flush` (#228) does that for the session being read: its prose held by `read_mode`, the
prose kept for its summary, its summaries in flight, waiting or settling and the decisions waiting
for them are dropped, and the rest of that reply is skipped: until the session's next `turn_start`,
its prose (also late prose after its `turn_end`) is dropped and its tool runs are not announced
(logged `dropped: flushed reply`), and it makes no summary. Its decisions (question, permission,
plan) asked later in that reply are still read, since they need an answer; `turn_done` still plays
when it ends. A flush answers nothing: a question of that session keeps its awaiting mark (its
permission prompt stays silent), and the decisions that waited for the cancelled summary are read
now instead of being dropped (decisions already queued in the channel are dropped with its other
unread text). With `flush_scope` `"session"` every other session keeps its held prose, its summary
work and its queued text, and is read next. With `"all"` every other session whose turn has ended
(its `turn_end` came since its last `turn_start`) also loses its ready messages: its unread text,
its summaries in flight, waiting or settling (the decisions that waited for them are read now), and
a channel without agent turn state (text a host spoke into it) loses its unread text too, also when
nothing was being read (text the background policy or a session mute holds). When anything of such a
session was dropped, the late prose of that reply (after its `turn_end`) is skipped too, as for the
session being read; its next reply is read as usual. In both scopes a session still writing its
reply (no `turn_end` yet) keeps everything: its queued and held text and its summary work, read when
done. A reply counts as still being written until its `turn_end`: a turn the user interrupted
(Claude Code fires no `Stop` for it) keeps its text in scope `all` until that session's next
`turn_start`. Mute (`mute_level`) is the way to silence every session.

Earcons are mixed over the speech (they never pause or cut it) and follow the output volume.

**One earcon at a time** (#238). Earcons are mixed over speech but never over each other: an earcon
triggered while another plays starts 60 ms after it ended, in the order they were triggered. A burst
is capped: an earcon equal to the one queued right before it is dropped, and at most 3 wait or play
at a time (the rest are dropped, with a line in `logs\sonarad.log`). An earcon still waiting when
`mute_level` becomes 2 is not played (a "dropped: mute level 2" line in the log) and the earcons
after an unmute do not wait for it; `stop` and `flush` leave waiting earcons alone. The `earcon`
event is sent when the earcon plays.

#### Custom earcons

`sonarad` plays `<home>\earcons\<kind>.wav` (for example `session_change.wav`, `turn_done.wav`)
instead of the bundled clip of that kind (#209). Any RIFF/WAVE file works: 8-, 16-, 24- or 32-bit
integer PCM or 32- or 64-bit float, plain or `WAVE_FORMAT_EXTENSIBLE`, mono or stereo (mixed down),
any sample rate (the output resamples), up to 10 seconds and 16 MB. A file is checked again (size
and modification time) each time its kind plays and on `get earcons`, so adding, replacing or
deleting one applies at the next play, without a restart or a reload command. A file that cannot be
used (not a WAV, an unsupported format, silent, too long or too big, unreadable) plays the bundled
clip instead, and one line in `logs\sonarad.log` says why (once per version of the file). The
`earcon` event names the kind either way.

### Settings

| key | value |
|---|---|
| `mute_level` | `0` (default), `1`: agent text is not spoken (what is queued and playing is dropped), earcons still play; `2`: earcons are silent too. Setting 1 or 2 stops everything queued and playing, core `speak` and channel `speak` items included (as the Python daemon's global mute); text spoken without the extension afterwards is read as usual. **Muting never loses a session's latest message** (runtime 0.20.2, #243): at level 1 and 2 the agent text that would be read is stored in its channel as that session's latest message, marked heard, so it is never read on its own, nothing is synthesized and no request reaches an engine. Setting 0 reads nothing. A switch to the session (`next_channel`) or `restart` (Up) reads it, from the top of the batch. Every rule runs as while unmuted (`read_mode`, send mode `message`, `turn_start`, `flush`), only the outcome is stored. Summaries are not made while muted: the prose kept for a summary is stored as it is (no summarizer run for text nobody hears); a summary that lands while muted is stored. Two exceptions to "unmuting reads nothing": a summary already in flight (or a settle window started) when muting that lands after unmuting is read, and with `read_mode` `done` prose held while muted is read by a `turn_end` that comes after unmuting (the turn ended unmuted). See "Agent batches" above |
| `verbosity` | `"everything"`: text, decisions, tool announcements, hints, and each code block announced as `"<n>-line <lang> code block"`; `"skip_code"` (default): text and decisions, code blocks dropped without a word, no tool announcements or hints. Aliases for older clients (#214): `"medium"` and `"quiet"` are `"skip_code"`, `"all"` is `"everything"`; a `set` with an alias answers (and saves) the new name, and a saved alias loads as the new name |
| `read_mode` | when a turn's sentences are read (#222): `"immediate"`: each as it arrives; `"queue"`: held until `minqueue` are waiting, the turn ends, a tool runs or a decision arrives; `"done"` (default): held until the turn ends or a decision (question, permission, plan) arrives, a tool run does not release them. A decision reads the held sentences first, then itself; after the turn ends, late sentences are read at once. With summaries on, summaries own the turn whatever the mode. A `config.json` from before #222 with a `minqueue` the user set and no `read_mode` loads `0` and `1` as `"immediate"` and more as `"queue"` (written with the next change); without one the default applies |
| `flush_scope` | what `control flush` (the flush hotkey) skips (#228): `"session"` (default): the session being read, for the rest of its reply, then the next session is read; `"all"`: also every other session's ready messages (queued text, finished replies, summaries); a reply still being written is kept in both. See `control flush` above |
| `minqueue` | `0` to `10` (default 5), read mode `"queue"` only: a turn's sentences are held until this many are waiting, the turn ends, a tool runs or a decision arrives; `0` and `1` read at once |
| `background_policy` | `"all"` (default) or `"earcon_only"`: see Background sessions |
| `summaries` | `{enabled, command, model, timeout, settle_ms, style, prompt, prompts, default_prompts}`: see below. `set` merges the fields given; `get` returns them all |
| `earcons` | read-only (`set` is `E_BAD_REQUEST`): `{folder, kinds, custom}`: `folder` is the custom earcons folder (`null` when the runtime has none), `kinds` every earcon kind, `custom` the kinds a usable file there replaces now. See Custom earcons |

**Summaries** (off by default). The turn's text is recorded instead of read; when the turn ends and
no text came for `settle_ms` (0 to 5000, default 600), a headless agent writes a spoken recap of it:
`command` `"claude"` (`claude -p`, tools and settings off) or `"codex"` (`codex exec`, read-only),
`model` (default `"haiku"`), `style` `"tidy"`, `"natural"` (default) or `"brief"`, or a custom
`prompt`. The command is found on `PATH` only and runs in the user's home folder with no window;
past `timeout` seconds (15 to 300, default 60) it is killed with its child processes. A turn shorter
than 280 characters is read as it is. A summary that fails or comes back empty falls back to the
turn's text. A decision waits for the recap of the text before it (read first), at most `timeout` +
5 s. Recaps are read in the order the turns ended, and one still out after twice `timeout` is read
as plain text. A new turn, an answer, `stop` or a `flush` of that session drops the recaps of the
channel still out. A runtime built without the summarizer answers `enabled: true` with
`E_UNSUPPORTED`.

**Custom prompts.** Each style can have its own instruction: `prompts` is `{style: text}` (`set`
changes the styles given; `null` or a blank text goes back to the built-in one), and `prompt` is the
custom instruction of the style in force (`set` with `prompt` changes that style's). Before #201
`prompt` was one instruction for every style; no runtime with that meaning shipped, so the protocol
minor was not bumped for it. `get` also returns `default_prompts`, the built-in instruction of each
style (read-only), so a page can show and edit it.

### Events

Stream `earcons` (ask for it by name in `subscribe`; `E_UNSUPPORTED` while the extension is off):
`{"event": "earcon", "kind": "turn_done"}` for every earcon played.

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

Spec section 4.4, L4 (`crates/sonara-system`, Windows). What happens to other apps' audio while
Sonara speaks, global hotkeys, spoken control cues and the settings page. Enable it with `hello`
`extensions: ["system"]`. Black-box tests: `conformance/system/` (with `--system fake`).

**Armed while needed.** Like every extension, `system` is enabled for the whole runtime once a
client asks for it, and its keys and the settings page work from then on. It acts on the PC (ducks
or pauses other apps, holds the hotkeys) only while it is **armed**: while a TCP client whose
`hello` asked for it is connected, or for good once a client asked for it with `keep_alive: true`
(over TCP or HTTP). When the last client that needed it disconnects, other apps are restored at once
and the hotkeys are released, even if the reader goes on reading. A plain HTTP request (the settings
page) never arms it.

**Other apps' audio.** With `audio_mode` `duck`, every other app's audio session on every active
output device is lowered to `duck_level` percent while an item is being read (playing, not paused,
not muted); with `pause`, media apps that are playing (Windows media transport controls) are paused
and later resumed. Never touched: the runtime's own process, the Windows audio engine
(`audiodg.exe`) and virtual mixers whose session is the whole mix (SteelSeries Sonar, VoiceMeeter);
an app already at or below the level is left alone. Other apps come back at once on `pause`, `mute`,
a mode change and when the extension is disarmed, about 0.4 s after the reader goes idle (so the gap
between two messages does not bring them up and down), and when the runtime exits.

**Crash restore.** Before an app is lowered or paused it is recorded in `state\duck_state.json` or
`state\pause_state.json` in the home, and the files are removed once everything is back. A runtime
that starts finds these files and restores the apps before it accepts clients; what still fails
stays recorded and is retried by the next restore.

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

`{"type": "shutdown"}` (#202) ends the runtime: what is reading or queued stops, later requests are
`E_BUSY`, and once the reply `{ok: true}` went out the process restores other apps' audio, releases
the hotkeys and exits, like an accepted takeover. It is how `sonara stop`, `sonara uninstall` and an
upgrade (`sonara start` of a newer release) end the Claude plugin's runtime. Before a client enabled
`system` it is `E_UNSUPPORTED`.

### Voice previews

`{"type": "preview", "voice"?: "<id or name>", "text"?: "<text>"}` says a short sample (default:
`"Hello. This is how Sonara sounds with this voice."`, at most 300 characters) with a voice of the
current engine (default: the voice in force) at the current rate, and replies `{engine, voice}` once
it is playing. The sample is synthesized on engines of its own (its own `onecore`; Kokoro's loaded
model is shared with the reader, so a Kokoro sample waits at most for the sentence being
synthesized, and a skip on the reader cancels it) and played as a clip mixed over whatever is being
read, like an earcon: nothing is paused, cut or queued again, and the output volume applies (a muted
reader plays it silently). An unknown voice is `E_NOT_FOUND`; before a client enabled `system` it is
`E_UNSUPPORTED`. With an [external engine](#external-engines) current, a preview is sent to it even
while Sonara is muted: the user asked for it.

### Spoken cues

Short confirmations, as the Python plugin spoke them (#197): the hotkeys say `"Paused."` /
`"Resumed."` (pause, only while an item is loaded), `"Muted."`, `"Super muted."`, `"Unmuted."` (mute
cycle; without `agent`, `"Muted."` / `"Unmuted."`), `"Rate 225."` (faster, slower) and
`"No session."` (`next_channel` with no channel open); a `set` of `mute_level` that changes it, of
`audio_mode` (`"Audio off."`, `"Audio ducking."`, `"Media pause."`) and of `duck_level`
(`"Duck level 40 percent."`) say theirs whoever sent it. A `rate` set from a page is not announced.
A cue is synthesized on the extension's own engines in the voice and at the rate in force and played
as a clip mixed over whatever is read, like a voice preview: it is heard while the reader is paused
and at every `mute_level`, never touches the queue, and a muted reader (core `mute`) plays it
silently. With an [external engine](#external-engines) current, the cues of a mute change
(`"Muted."`, `"Super muted."`, `"Unmuted."`) and every cue said while Sonara is muted are spoken
with the built-in engine, never sent to it (#227). Cues are spoken in order; a rate, duck-level or
audio-mode cue still waiting when a newer one of the same kind comes is skipped. Stream `cues` (ask
for it by name in `subscribe`, or `GET /v1/events?events=cues`; `E_UNSUPPORTED` over TCP while the
extension is off): `{"event": "cue", "text": "Muted."}` for every cue spoken. The Python plugin's
setup-guide cue ("run slash sonara install") is not carried over: the runtime needs no install step
(the hook starts it).

### Hotkeys

Actions and what they do (the same as the protocol request named):

| action | default | effect |
|---|---|---|
| `restart` | Ctrl+Alt+Up | `control restart` |
| `flush` | Ctrl+Alt+Down | with `channels`: `control flush` (the session being read and the rest of its reply, #228; with `flush_scope` `all` also every other session's ready messages); without it, `control stop`. With `agent`, plays `nav` (or `nav_edge` when nothing was dropped) |
| `pause` | unbound | `control toggle`, then the cue `"Paused."` or `"Resumed."` |
| `mute` | Ctrl+Alt+M | with `agent`: `mute_level` 0, 1, 2, 0 and so on; else `mute` and `unmute`; then its cue |
| `next_channel` | Ctrl+Alt+P | `control next_channel` (with `channels`); with `agent`, plays `session_change` then says `"Session changed: <label>."` (the announcement; `"Session changed."` for a session without a label; when switches are not announced, the earcon alone); with no channel open, the cue `"No session."` |
| `faster` / `slower` | unbound | `rate` plus or minus 25, then `"Rate N."` |

A binding is a `key` (a letter, a digit, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`,
`pagedown`, `period`, `leftbracket`, `rightbracket`) and `mods` (`ctrl`, `alt`, `shift`, `win`). A
hotkey must hold Ctrl, Alt or Win (`E_BAD_REQUEST` otherwise: it would take that key away from every
app); an unknown key, modifier or action is `E_BAD_REQUEST`, and nothing is written. Holding a key
does not repeat an action, and a second press of `pause` or `mute` within 0.3 s is ignored. Hotkeys
do nothing once a takeover was accepted.

The defaults use Ctrl+Alt, which is AltGr on many European keyboard layouts: a hotkey that is AltGr
typing a character is reported in `altgr`, and the fix is a binding with Win (Windows itself owns
Win+Alt+Up/Down/M/P). The user's bindings are kept in `keymap.json` in the home (only the overrides;
`nav_start` and `next_session` from the Python plugin's keymap are read as `restart` and
`next_channel`).

`get hotkeys` value:

```json
{"active": true,
 "bindings": [{"action": "restart", "key": "up", "mods": ["ctrl", "alt"], "combo": "Ctrl+Alt+Up", "registered": true, "error": null, "altgr": null},
              {"action": "mute", "key": "m", "mods": ["ctrl", "alt"], "combo": "Ctrl+Alt+M", "registered": true, "error": null, "altgr": "µ"},
              {"action": "pause", "key": null, "mods": [], "combo": null, "registered": false, "error": null, "altgr": null}],
 "keys": ["0", "1", "...", "up"], "mods": ["alt", "cmd", "control", "ctrl", "shift", "win"], "problems": []}
```

`active`: the hotkeys are registered now (the extension is armed). `registered: false` with
`error: "already_owned"` means another program owns that chord. `problems` lists entries of
`keymap.json` that were skipped: an unknown key or modifier never disables the other hotkeys.

### Settings page

`GET /settings?token=<token>` on the HTTP port (the `settings_url`) serves the settings page, on
this API. Since 0.20.0 (#237) it is one dark page with a sidebar (search, the sections, and a "Now"
card that says which engine reads and how it is doing, from `runtime.engine_status`), labels without
helper text, a small "Saved" toast and one polite live region: Speech (the voice engine's status
from `runtime.engine_status`, voice with previews, rate, mute level, Detail (`verbosity`), Other
sessions (`background_policy`)), Summary (mode Off, Tidy, Natural or Brief, the model, the prompt of
the style in force (Default or Custom, Reset), live reading: `read_mode` Immediately, Queue with its
`minqueue` stepper shown only in Queue, or When done, gated while a summary mode is on; Advanced:
summary timeout and settle time), Audio (speech volume, other apps, duck level shown only for Duck,
the folder for your own chimes and which ones are in use), Sessions (`channel_prefs`: name and audio
per open session; "Earlier (n)", the closed sessions with saved preferences, each with Forget
(`channel_prefs` `forget`); switch announcements), Hotkeys (capture, unbind, reset; a chord another
program owns or Windows refused is shown at its row, an AltGr clash only by `sonara doctor`; Flush
skips: This session or Everything queued, the `flush_scope`), Engines (only when
`hello.capabilities` has `engines`, since 0.18.0, #227: the `engine_list` profiles with where each
sends text, key state and status, Use (`set engine`), Test (`engine_test` with `play: true`, the
reply's `ms` or the error with its `reason`), Edit and Remove (`engine_remove`, after a
confirmation); an add/edit form in a modal slide-over drawer (the page behind is inert, Escape or
the scrim closes it, asking first when there are unsaved changes) that sends `engine_add`
(`replace: true` when editing, `secret` only when the password field is filled, which empties after
the save; `url` only when set and not the preset's own, `send_mode` only once picked in "Send to the
engine" (Full message in one request, or As it comes in; the provider's default is preselected,
#235), `model` and `voice` when chosen (nothing is preselected: since 0.19.0, #235, the page names
no model or voice; the model is a list from `engine_models` with "Other model id", or a typed id
where the provider has no list, and the voice a list with "Other voice id"; both starred where
required), the form starting from the view's `explicit`). The form marks required fields with a
star, starts the label at the provider's name and makes the ID from it (lowercase, `a-z0-9-_`,
unique, editable), checks the fields before sending and shows each problem (and a refusal of the
runtime) next to its field, says when it is saved or has unsaved changes, and tests only the saved
engine. Its voice and model lists load before a save from `voices {profile, secret}` (protocol 1.4)
and `engine_models {profile, secret}` (protocol 1.5) as soon as the provider and key are filled in,
each voice named with its provider ("Adam (ElevenLabs)"), and from `voices {engine, refresh: true}`
when editing) and System (version, uptime, protocol, the settings file, the debug log). Since #227
(0.18.0) the Speech section's engine is a dropdown again: the built-in engines (Kokoro, Windows
voices), every usable added engine by its label, and a last entry "Add new engine" that opens the
Engines form; choosing one sends `set engine`, the voice list follows it (`voices {engine}`), and
the status line under it says how it is doing. The page polls `engine_list` every 3 s, which writes
no `in` line to the troubleshooting log. The per-session `voice` of `channel_prefs` is stored but
not applied, so the page does not show it. The agent sections say so while no client enabled
`agent`. It uses only the HTTP API above, with the token filled in by the runtime. It is answered
only while `system` is enabled (`404` before), only with the token (`401`) and only for the `Host`
`127.0.0.1:<http_port>` or `localhost:<http_port>` (`403`, against DNS rebinding). Its fonts (Geist
and Geist Mono, SIL OFL 1.1) are compiled into sonarad and inlined as `data:` URLs, so it loads
nothing from another origin. It is sent with `Content-Security-Policy` (`frame-ancestors 'none'`,
connections to itself only, images and fonts only as `data:`), `Referrer-Policy: no-referrer` and
`Cache-Control: no-store`. The API sends no CORS headers, so other origins cannot read its replies.

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
