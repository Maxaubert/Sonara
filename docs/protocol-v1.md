# Sonara protocol v1

The contract between `sonarad.exe` (the Sonara runtime) and its clients: apps that bundle Sonara, SDKs (`@sonara/client`, `sonara-client`) and anything else on the same PC. This document covers the **core** (protocol 1.0), which every runtime offers. Extensions (`channels`, `agent`, `system`) are reserved and listed at the end; until a runtime offers them, their messages return `E_UNSUPPORTED`.

Source of truth in code: `crates/sonarad` (server), `crates/sonara-reader` (the reader behind it). Black-box tests: `conformance/` (`python -m pytest conformance -q` after `cargo build -p sonarad`). Spec: `docs/plans/2026-10-02-sonara-runtime-spec.md` sections 3 and 4.

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
  "version": "0.9.4",
  "protocol": {"major": 1, "minor": 0},
  "capabilities": ["core", "speak", "control", "set", "get", "voices", "subscribe", "events.state", "events.items", "events.log"],
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
- The **first message must be `hello` with the token**. Anything else (another type, a wrong token, invalid JSON) is answered with `E_AUTH` and the connection closes. A `hello` that fails for another reason (`E_UNSUPPORTED`, `E_INCOMPATIBLE`, `E_BUSY`) leaves the connection open and unauthenticated, so the client may send `hello` again.
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
| `require[]` | capabilities or extensions the client cannot work without; any missing is `E_UNSUPPORTED` |
| `extensions[]` | extensions the client would like enabled; those this runtime lacks are listed in `unavailable`, not an error |
| `takeover?` | `true`: ask the runtime to exit for a newer one (see [Takeover](#takeover)) |
| `keep_alive?` | `true`: the runtime keeps running after the last client left (until a takeover or the process is ended) |

```json
> {"type": "hello", "id": 1, "token": "...", "client": {"name": "prism", "version": "2.1"}, "protocol": {"major": 1, "minor": 0}, "require": ["core"], "extensions": ["channels"]}
< {"id": 1, "ok": true, "version": "0.9.4", "protocol": {"major": 1, "minor": 0}, "capabilities": ["core", "speak", ...], "extensions": [], "unavailable": ["channels"]}
```

**Capabilities** of protocol 1.0: `core`, `speak`, `control`, `set`, `get`, `voices`, `subscribe`, `events.state`, `events.items`, `events.log`. A later minor adds capability strings for what it adds, so a client can `require` them.

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
| `engine` | an engine id (`onecore`; `fake` in test runs). Switching resets a voice the new engine lacks |

A rate, voice or engine change applies to chunks synthesized from then on. Out-of-range or wrongly typed values are `E_BAD_REQUEST`; an unknown voice or engine is `E_NOT_FOUND`; an unknown key is `E_BAD_REQUEST` (an extension's key, such as `audio_mode`, is `E_UNSUPPORTED`).

### `voices`

`{"type": "voices", "engine?": "<id>"}` replies `{ok: true, voices: [...]}` for one engine or all:

```json
{"id": "...", "name": "Microsoft Zira", "language": "en-US", "engine": "onecore", "license_class": "os", "installed": true}
```

`license_class` is `permissive` or `os`. `installed: false` means listed but not yet able to speak (voice data missing, a model still to download). An unknown engine is `E_NOT_FOUND`.

### `subscribe` (TCP)

`{"type": "subscribe", "events": ["state", "items", "log"]}` (omitted: all three) replies `{ok: true, events: [...]}` and from then on sends those events on this connection. Subscribing again replaces the set (`[]` stops events). An unknown stream name is `E_UNSUPPORTED`. When `state` is included, the first event is the current state.

A client that does not read its events never slows the reader: past 256 unread events, events are dropped for that client. Each `state` event is a full snapshot, so the next one brings a player up to date.

## Events

```json
{"event": "state", "seq": 12, "now_playing": {"item_id": 7, "label": "build", "text": "Build finished.", "chunk": 0, "chunks": 2}, "queued": 0, "paused": false, "muted": false, "volume": 100, "rate": 200, "voice": null, "engine_status": {"engine": "onecore"}}
{"event": "item", "item_id": 7, "phase": "started"}
{"event": "log", "message": "synthesis failed: ..."}
```

- `state` (stream `state`): sent on change only, `seq` strictly increasing. `now_playing` is `null` when idle; its `text` is the chunk being read. `queued` counts items after the current one. `engine_status` names the current engine; readiness and model download progress are added by a later minor.
- `item` (stream `items`): `phase` is `started`, `finished`, `skipped` or `failed`. An item ends `failed` only when none of its chunks could be played; a failed chunk is skipped and logged.
- `log` (stream `log`): a line worth showing in a log, such as a failed synthesis or an engine that is not ready.

## Errors

| code | when |
|---|---|
| `E_AUTH` | missing or wrong token; on TCP a first message other than `hello` (the connection closes) |
| `E_BAD_REQUEST` | not a JSON object, missing `type`, a missing or wrongly typed field, an out-of-range value, an unknown action or key |
| `E_UNKNOWN_TYPE` | a `type` this protocol does not define |
| `E_UNSUPPORTED` | an unmet `require`, an unknown event stream, or a message, action or key of an extension that is not enabled |
| `E_INCOMPATIBLE` | `hello` with another protocol major |
| `E_BUSY` | `hello` with `takeover: true` while something is playing or queued |
| `E_ENGINE` | the engine or the reader failed |
| `E_NOT_FOUND` | an unknown voice or engine |

## Lifetime

A client is a TCP connection that completed `hello`, or an open SSE stream; a plain HTTP request only counts as activity. The runtime exits 30 s (`--idle-exit <seconds>`) after the last client left and nothing is being read (a paused item does not count), unless a client sent `keep_alive: true` or it runs with `--standalone`. Ctrl+C ends it cleanly. On every clean exit it stops speech and removes `runtime.json`.

## Takeover

When a client finds an instance it cannot use (another protocol major, a missing capability):

1. It sends `hello` with the token and `takeover: true`.
2. If nothing is playing or queued (a paused item counts as busy), the runtime replies `{ok: true, takeover: true, ...}`, closes the connection, stops audio, removes `runtime.json` and exits with code 0. The client waits for the process to end, then starts its bundled runtime.
3. Otherwise the reply is `E_BUSY`; the client retries after the current item (bounded, 30 s in total), then gives up with `E_INCOMPATIBLE`.

## Versioning

Semantic versioning on `protocol: {major, minor}`. A minor only adds optional fields, message types, capabilities and events; it never changes the meaning of what exists. Clients ignore unknown fields and event types. A new major is a new protocol: a client that needs it takes over an idle older runtime.

## Testing aids

`sonarad --engine fake` uses a deterministic tone engine (10 ms of audio per character at rate 200; text containing `[fail]` fails to synthesize) and, unless `--output device` is given, `--output null`: a silent output that keeps real time. They are meant for tests and the conformance suite, not for apps.

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

## Extensions (reserved)

Spec sections 4.2 to 4.4. A client asks for them in `hello.extensions`; this runtime offers none yet, so their messages return `E_UNSUPPORTED`.

| extension | adds |
|---|---|
| `channels` | `channel?` on `speak`/`control`; `channel_open`, `channel_close`, `focus`; `control` `next_channel`; `state.now_playing.channel`, `host_tab` |
| `agent` (needs `channels`) | `stream`, `turn_start`, `turn_end`, `ask`, `earcon`; `set mute_level`, `set summaries` |
| `system` | `set audio_mode`, `set duck_level`, `set hotkeys`, `get settings_url` |
