# Sonara protocol v1

The contract between `sonarad.exe` (the Sonara runtime) and its clients: apps that bundle Sonara, SDKs (`@sonara/client`, `sonara-client`) and anything else on the same PC. This document covers the **core** (protocol 1.1), which every runtime offers, and the [`channels`](#extension-channels), [`agent`](#extension-agent) and [`system`](#extension-system) extensions.

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
  "protocol": {"major": 1, "minor": 1},
  "capabilities": ["core", "speak", "control", "set", "get", "voices", "subscribe", "events.state", "events.items", "events.log", "engine_status"],
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
| `engine` | an engine id: `kokoro` or `onecore` (`fake` in test runs). Switching resets a voice the new engine lacks |

A rate, voice or engine change applies to chunks synthesized from then on. Out-of-range or wrongly typed values are `E_BAD_REQUEST`; an unknown voice or engine is `E_NOT_FOUND`; an unknown key is `E_BAD_REQUEST` (an extension's key, such as `audio_mode`, is `E_UNSUPPORTED`).

### `voices`

`{"type": "voices", "engine?": "<id>"}` replies `{ok: true, voices: [...]}` for one engine or all:

```json
{"id": "...", "name": "Microsoft Zira", "language": "en-US", "engine": "onecore", "license_class": "os", "installed": true}
```

`license_class` is `permissive` or `os`. `installed: false` means listed but not yet able to speak (voice data missing, a model still to download). An unknown engine is `E_NOT_FOUND`.

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
| `E_ENGINE` | the engine or the reader failed |
| `E_NOT_FOUND` | an unknown voice or engine |

## Lifetime

A client is a TCP connection that completed `hello`, or an open SSE stream; a plain HTTP request only counts as activity. The runtime exits 30 s (`--idle-exit <seconds>`) after the last client left and nothing is being read (a paused item does not count), unless a client sent `keep_alive: true` or it runs with `--standalone`. The countdown starts when `runtime.json` is written, so a slow start never uses it up before the first client can connect. The exit is decided atomically with the requests that start speech: a `speak` or `control` either comes first (and keeps the runtime while it is read) or is answered `E_BUSY`, never accepted and then lost; any request that reaches the runtime while it exits is `E_BUSY` (#194). Ctrl+C and a [`shutdown`](#shutdown) (extension `system`) end it cleanly. On every clean exit it stops speech and removes `runtime.json`.

## Takeover

When a client finds an instance it cannot use (another protocol major, a missing capability):

1. It sends `hello` with the token and `takeover: true`.
2. If nothing is playing or queued (a paused item counts as busy), the runtime replies `{ok: true, takeover: true, ...}` and from then on answers `speak` and `control` from any client with `E_BUSY`, so nothing is accepted and then dropped. It closes the connection, stops audio, releases the single-instance lock, removes `runtime.json` and exits with code 0. The client waits for the process to end (or, at the least, for `runtime.json` to go), then starts its bundled runtime.
3. Otherwise the reply is `E_BUSY`; the client retries after the current item (bounded, 30 s in total), then gives up with `E_INCOMPATIBLE`.

## Versioning

Semantic versioning on `protocol: {major, minor}`. A minor only adds optional fields, message types, capabilities and events; it never changes the meaning of what exists. 1.1 (runtime 0.10.0) added the readiness fields of `engine_status` and the `engine_status` capability. Clients ignore unknown fields and event types. A new major is a new protocol: a client that needs it takes over an idle older runtime.

## Saved settings

Every setting a client changes with `set` is saved in the home and applies again when the next runtime starts (#201): the core keys before the runtime accepts its first client (so before the first speech), an extension's keys when a client enables that extension. Hotkeys that change a setting (the mute cycle, faster, slower) save it too.

**Defaults** (#202). A setting nobody set has the runtime's default, the Claude plugin's product settings: `voice` `af_sarah` (with an engine that has it; otherwise the engine's default voice), `rate` 250, `volume` 100, `channel_announce` `"on"`, `mute_level` 0, `verbosity` `"medium"`, `minqueue` 5, `background_policy` `"all"`, summaries off, `audio_mode` `"pause"`, `duck_level` 30. The tables below give the same defaults. `sonarad` applies them to the layers itself; the library crates keep their own (rate 200, `"everything"`, 1, `"earcon_only"`, `"off"`).

| file in the home | holds |
|---|---|
| `config.json` | only the keys a client set (even to the default), never the defaults: `engine`, `voice`, `rate`, `volume`, `channel_announce`, `mute_level`, `verbosity`, `minqueue`, `background_policy`, `summaries` (only the fields that were set, plus `prompts`), `audio_mode`, `duck_level`. A `_migrated` key records the migration below |
| `session_prefs.json` | per channel: `label`, `voice`, `muted` (see `channel_prefs`), the 200 most recently changed |
| `keymap.json` | the hotkey overrides (see [Hotkeys](#hotkeys)) |
| `earcons\` | the user's own earcons: `<kind>.wav` replaces that kind's bundled clip (see [Custom earcons](#custom-earcons)); created empty at start, only read |
| `logs\sonarad.log` | one line per start (`sonarad <version> started (pid <pid>): engine <id> <status>; home <dir>`), the engine's readiness changes (model downloaded, loaded or failed; not the progress), the migration, saved values that could not be applied, custom earcons used or refused |

Files are written atomically (a temp file, then a rename). A `config.json` that is not a JSON object gives the defaults and is copied to `config.json.bad` (and logged) before the next save replaces it. A value out of range is not applied (and logged), the others still apply; it stays in the file, like a key this runtime does not know (from a newer release), until a client sets that key. A saved value the reader refuses at start, such as a voice the current engine lacks (a Kokoro voice from the Python plugin while only OneCore is installed), is logged and kept in `config.json`, so it applies once it is available; the default is used meanwhile. `--engine` on the command line wins over a saved `engine`, which wins over the default choice below; a saved engine that cannot start is logged and the default choice is used. Setting `engine` to another engine replaces a saved voice that engine lacks with the voice in force; setting the same engine again keeps it.

**Migration from the Python plugin.** The first runtime on the default home (`%LOCALAPPDATA%\Sonara`) that has no `config.json` imports the plugin's settings from `%USERPROFILE%\.sonara` (another folder, or another home: `--migrate-from <dir>`): its `config.json` (voice, rate, speech volume, audio mode, duck level, mute level, verbosity, minimum queue, background policy (`earcon_only`, or any other value as `all`), summary mode, command, model, timeout, settle time, style and custom prompts), `keymap.json` (`nav_start` becomes `restart`, `next_session` becomes `next_channel`; only when the home has no `keymap.json`) and `session_prefs.json` (the session `name` becomes `label`; only when the home has none), and the user's own earcons (`config.json` `earcons: {kind: path}`; each file that exists is copied to `earcons\<kind>.wav`, unless one is there; paths to the plugin's bundled WAVs are skipped). Every value of a format 2 file (the plugin saved only what the user set) is saved, also one equal to the runtime's default, so a user who chose the plugin's old default keeps it; in a file from before format 2 (every key written) a value equal to the plugin's old default (rate 200, volume 100, `audio_mode` `"off"`, `mute_level` 0, `verbosity` `"everything"`, `minqueue` 1, `background_policy` `"earcon_only"`) counts as unset. Summary fields equal to the runtime's default are not saved; a Chatterbox voice speaks as `af_heart`, `audio_control: true` is `audio_mode: duck`, a speech volume above 100 is 100, and in a file from before the plugin's format 2 the old defaults `duck_level: 20` and `summary_timeout: 20` count as unset. The cue voice and fast cues are not imported (the runtime speaks its control cues in the voice in force). The plugin's folder is only read. The migration writes `config.json` with the `_migrated` marker, so it runs once; what it did is in `logs\sonarad.log`.

## Testing aids

`sonarad --engine kokoro|onecore` picks the engine to start with; without it, the saved `engine`, else `kokoro` when `onnxruntime.dll` is next to `sonarad.exe` (`SONARA_ORT_DYLIB` names another copy, a development aid), else `onecore`. A Kokoro start verifies or downloads and loads the model in the background right away. `sonarad --engine fake` uses a deterministic tone engine (10 ms of audio per character at rate 200; text containing `[fail]` fails to synthesize) and, unless `--output device` is given, `--output null`: a silent output that keeps real time. `sonarad --system fake` replaces the Windows side of the [`system`](#extension-system) extension with fake apps, media sessions, hotkeys and keyboard layout kept in `<home>\fake-system.json` (read and written on every operation), so a test can set up the "apps", kill the runtime and check what the next one restores. They are meant for tests and the conformance suite, not for apps. With `--engine fake` there is no Kokoro engine, so nothing is ever downloaded.

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
| `control` | `channel?` plus the core `action`, or `action: next_channel` | see below |

`channel` is a non-empty string (`E_BAD_REQUEST` otherwise); an unknown channel in `channel_close`, `focus` or `control` is `E_NOT_FOUND`.

**`control` once `channels` is enabled.** Without `channel` the actions are the core ones, except:

- `stop` also skips every channel to its end (nothing more is read until a new message; heard messages stay replayable).
- `restart` while nothing is playing replays the batch of the channel being read or read last (the Claude plugin's Up key), not only its last item.

With `channel`: `stop` skips that channel to its end and cuts its item if it is being read; `restart` goes back to the start of its item if that channel is being read, else replays the channel's batch from the top and switches to it (cutting the current item, announced); any other action applies only while that channel is being read, and is a no-op otherwise.

`next_channel` (reply `{channel}`, `null` when no channel is open) switches now: it moves around the channels in opening order, skipping channels with nothing to hear (unless all are empty), starting from the channel being read or the one that read last. It cuts the current item and announces the target. A fully heard target, landing on the same channel, or returning to a replay in progress replays the batch from the top; unread messages resume where they stopped (a message cut by the switch is read again).

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
| `turn_end` | `channel`, `turn?`, `t?` | the agent finished: plays `turn_done` and reads text held by `minqueue`. Reply `{stale}` |
| `ask` | `channel`, `kind: question\|permission\|plan`, `text?`, `options?`, `multi_select?`, `notes?`, `hint?`, `hint_once?` | a decision, read with priority (after the item playing, before the other channels). `question`: the text, then `Option n: label.` and its description for each of `options` (strings or `{label, description?}`; an option without a label keeps its number), plays `choice`, and marks the channel as waiting for an answer. `permission`: the pending action, plays `permission`; while a question waits, the permission prompt it fires itself is dropped (no earcon, no text) and clears the mark. `plan`: `"Plan ready. <text>"`, no earcon. `notes` is read after the decision; `hint` too at verbosity `everything`, and `hint_once` after it the first time a channel gets one |
| `tool` | `channel`, `name`, `summary?` | the agent runs a tool: clears a waiting question; at verbosity `everything` it reads `summary` (else `"Running <name>."`) after the text held so far |
| `answered` | `channel` | the user answered the question: everything queued for the channel is stale, so its unread text is dropped and its item cut, summary work and held decisions are dropped; the turn goes on |
| `earcon` | `kind` | play an earcon: `choice`, `permission`, `error`, `turn_done`, `nav`, `nav_edge`, `session_change`, `summary_failed` |

`channel` is a non-empty string (`E_BAD_REQUEST` otherwise); a channel is opened with the defaults when it gets its first text (open it with `channel_open` to give it a label for announcements). With the extension on, `channel_close` also forgets the channel's turn, and `control stop` without a channel also drops every channel's summary work and held decisions.

Earcons are mixed over the speech (they never pause or cut it) and follow the output volume.

#### Custom earcons

`sonarad` plays `<home>\earcons\<kind>.wav` (for example `session_change.wav`, `turn_done.wav`) instead of the bundled clip of that kind (#209). Any RIFF/WAVE file works: 8-, 16-, 24- or 32-bit integer PCM or 32- or 64-bit float, plain or `WAVE_FORMAT_EXTENSIBLE`, mono or stereo (mixed down), any sample rate (the output resamples), up to 10 seconds and 16 MB. A file is checked again (size and modification time) each time its kind plays and on `get earcons`, so adding, replacing or deleting one applies at the next play, without a restart or a reload command. A file that cannot be used (not a WAV, an unsupported format, silent, too long or too big, unreadable) plays the bundled clip instead, and one line in `logs\sonarad.log` says why (once per version of the file). The `earcon` event names the kind either way.

### Settings

| key | value |
|---|---|
| `mute_level` | `0` (default), `1`: agent text is not spoken (what is queued and playing is dropped), earcons still play; `2`: earcons are silent too. Setting 1 or 2 stops everything queued and playing, core `speak` and channel `speak` items included (as the Python daemon's global mute); text spoken without the extension afterwards is read as usual |
| `verbosity` | `"everything"`: text, decisions, tool announcements and hints; `"medium"` (default): no tool announcements or hints; `"quiet"`: decisions only |
| `minqueue` | `0` to `10` (default 5): a turn's sentences are held until this many are waiting, the turn ends, a tool runs or a decision arrives; `0` and `1` read at once |
| `background_policy` | `"all"` (default) or `"earcon_only"`: see Background sessions |
| `summaries` | `{enabled, command, model, timeout, settle_ms, style, prompt, prompts, default_prompts}`: see below. `set` merges the fields given; `get` returns them all |
| `earcons` | read-only (`set` is `E_BAD_REQUEST`): `{folder, kinds, custom}`: `folder` is the custom earcons folder (`null` when the runtime has none), `kinds` every earcon kind, `custom` the kinds a usable file there replaces now. See Custom earcons |

**Summaries** (off by default). The turn's text is recorded instead of read; when the turn ends and no text came for `settle_ms` (0 to 5000, default 600), a headless agent writes a spoken recap of it: `command` `"claude"` (`claude -p`, tools and settings off) or `"codex"` (`codex exec`, read-only), `model` (default `"haiku"`), `style` `"tidy"`, `"natural"` (default) or `"brief"`, or a custom `prompt`. The command is found on `PATH` only and runs in the user's home folder with no window; past `timeout` seconds (15 to 300, default 60) it is killed with its child processes. A turn shorter than 280 characters is read as it is. A summary that fails or comes back empty falls back to the turn's text. A decision waits for the recap of the text before it (read first), at most `timeout` + 5 s. Recaps are read in the order the turns ended, and one still out after twice `timeout` is read as plain text. A new turn, an answer or `stop` drops the recaps of the channel still out. A runtime built without the summarizer answers `enabled: true` with `E_UNSUPPORTED`.

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
| `runtime` | read-only: `{pid, uptime_s, http_port, config, previews, saved_voice}`; `config` is the path of the saved settings (`null` when the runtime saves none), `previews` whether `preview` works, `saved_voice` the voice saved in `config.json` (`null`: none), which differs from `voice` while the engine lacks it |

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
| `flush` | Ctrl+Alt+Down | `control stop` (everything queued or playing); with `agent`, plays `nav` (or `nav_edge` when there was nothing) |
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

`GET /settings?token=<token>` on the HTTP port (the `settings_url`) serves the settings page of the Python plugin, on this API: Speech (engine, voice with previews, rate, mute level, verbosity, background sessions), Summary (mode Off, Tidy, Natural or Brief, the instruction of each style, the model, the minimum queue), Audio (speech volume, other apps, duck level, the folder for your own chimes and which ones are in use), Sessions (`channel_prefs`: name, audio, voice per session; switch announcements), Hotkeys (capture, unbind, reset, AltGr and ownership warnings), Advanced (summary timeout and settle time) and System (version, protocol, process, uptime, port, extensions, engines, the settings file). The agent sections say so while no client enabled `agent`. It uses only the HTTP API above, with the token filled in by the runtime. It is answered only while `system` is enabled (`404` before), only with the token (`401`) and only for the `Host` `127.0.0.1:<http_port>` or `localhost:<http_port>` (`403`, against DNS rebinding). It is sent with `Content-Security-Policy` (`frame-ancestors 'none'`, connections to itself only), `Referrer-Policy: no-referrer` and `Cache-Control: no-store`. The API sends no CORS headers, so other origins cannot read its replies.

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
