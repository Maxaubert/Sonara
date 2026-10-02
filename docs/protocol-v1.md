# Sonara protocol v1

The contract between `sonarad.exe` (the Sonara runtime) and its clients: apps that bundle Sonara, SDKs (`@sonara/client`, `sonara-client`) and anything else on the same PC. This document covers the **core** (protocol 1.0), which every runtime offers, and the [`channels`](#extension-channels) and [`agent`](#extension-agent) extensions. The `system` extension is reserved and listed at the end; until a runtime offers it, its messages return `E_UNSUPPORTED`.

Source of truth in code: `crates/sonarad` (server), `crates/sonara-reader` (the reader behind it), `crates/sonara-channels` (the `channels` extension). Black-box tests: `conformance/` (`python -m pytest conformance -q` after `cargo build -p sonarad`). Spec: `docs/plans/2026-10-02-sonara-runtime-spec.md` sections 3 and 4.

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
  "version": "0.9.5",
  "protocol": {"major": 1, "minor": 0},
  "capabilities": ["core", "speak", "control", "set", "get", "voices", "subscribe", "events.state", "events.items", "events.log"],
  "extensions": ["channels", "agent"],
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
< {"id": 1, "ok": true, "version": "0.9.5", "protocol": {"major": 1, "minor": 0}, "capabilities": ["core", "speak", ...], "extensions": ["channels"], "unavailable": []}
```

The reply's `extensions` lists the extensions enabled on this runtime now. An extension is enabled for the whole runtime as soon as any client asks for it (in `extensions` or `require`) and stays enabled until the runtime exits; until then its messages, actions and keys are `E_UNSUPPORTED`. `runtime.json` lists in `extensions` the ones this runtime offers.

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

`{"type": "subscribe", "events": ["state", "items", "log"]}` (omitted: all three) replies `{ok: true, events: [...]}` and from then on sends those events on this connection. Subscribing again replaces the set (`[]` stops events). An unknown stream name is `E_UNSUPPORTED`; an extension's stream (`earcons` of `agent`) is asked for by name and is `E_UNSUPPORTED` while the extension is off. When `state` is included, the first event is the current state.

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
| `E_AUTH` | missing or wrong token; on TCP a first message other than `hello`, or no `hello` within 5 s (the connection closes) |
| `E_BAD_REQUEST` | not a JSON object, missing `type`, a missing or wrongly typed field, an out-of-range value, an unknown action or key |
| `E_UNKNOWN_TYPE` | a `type` this protocol does not define |
| `E_UNSUPPORTED` | an unmet `require`, an unknown event stream, or a message, action or key of an extension that is not enabled |
| `E_INCOMPATIBLE` | `hello` with another protocol major |
| `E_BUSY` | `hello` with `takeover: true` while something is playing or queued; `speak` or `control` after a takeover was accepted |
| `E_ENGINE` | the engine or the reader failed |
| `E_NOT_FOUND` | an unknown voice or engine |

## Lifetime

A client is a TCP connection that completed `hello`, or an open SSE stream; a plain HTTP request only counts as activity. The runtime exits 30 s (`--idle-exit <seconds>`) after the last client left and nothing is being read (a paused item does not count), unless a client sent `keep_alive: true` or it runs with `--standalone`. Ctrl+C ends it cleanly. On every clean exit it stops speech and removes `runtime.json`.

## Takeover

When a client finds an instance it cannot use (another protocol major, a missing capability):

1. It sends `hello` with the token and `takeover: true`.
2. If nothing is playing or queued (a paused item counts as busy), the runtime replies `{ok: true, takeover: true, ...}` and from then on answers `speak` and `control` from any client with `E_BUSY`, so nothing is accepted and then dropped. It closes the connection, stops audio, releases the single-instance lock, removes `runtime.json` and exits with code 0. The client waits for the process to end (or, at the least, for `runtime.json` to go), then starts its bundled runtime.
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

## Extension `channels`

Spec section 4.2, L2 (`crates/sonara-channels`). Several named sources (terminal tabs, chats) share the one reader: each **channel** keeps its own messages and policy, and one channel is read at a time. Enable it with `hello` `extensions: ["channels"]`. Black-box tests: `conformance/channels/`.

**Model.** A channel holds its current **batch**: the messages sent to it since it was last caught up, with a read position. Messages wait in their channel and go to the reader one at a time, only when the reader is idle, so text spoken without a `channel` (core `speak`) is read first. Heard messages stay, so a manual return can replay the batch; a message sent to a channel that is caught up (and not still reading its last message) starts a new batch.

- **Policy** `latest` (the default): a new message replaces the channel's unread messages, so the newest one is always read and never dropped ("one message, always the last"). `queue`: every message is read, in order.
- **Who reads next:** the channel being read keeps the floor until its batch is read; then the focused channel; then the first channel (in opening order) with something unread. A channel you left with `next_channel` is not resumed on its own until it gets a new message.
- **Announcements:** a switch to another channel is announced by a short item before its first message: `"<label>."`, or `"<label>, reading again."` when the batch is replayed from the top. An automatic hand-off is announced when the channel differs from the one that read last (never for the first channel to read); `next_channel` is always announced. A channel without a `label` is not announced. `set channel_announce "off"` turns announcements off.

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

### State

`state.now_playing` gains `channel` and `host_tab` (both `null` for text spoken without a channel; an announcement belongs to the channel it announces), and `queued` also counts the channels' unread messages. A `state` event is sent when the reader's state changes, so `queued` catches up with a new channel message at the next change.

```json
{"event": "state", "seq": 31, "now_playing": {"item_id": 12, "label": "Build tab", "text": "Build finished.", "chunk": 0, "chunks": 1, "channel": "tab-3", "host_tab": "3"}, "queued": 1, "paused": false, "muted": false, "volume": 100, "rate": 200, "voice": null, "engine_status": {"engine": "onecore"}}
```

## Extension `agent`

Spec section 4.3, L3 (`crates/sonara-agent`). Speech for coding agents and chat assistants on top of [`channels`](#extension-channels): one channel per agent session, with streamed text, turns, decisions spoken with priority, earcons, three mute levels and optional summaries. Enable it with `hello` `extensions: ["agent"]`; it needs `channels`, which is enabled with it (the reply lists both). Black-box tests: `conformance/agent/`.

**Model.** Each channel has a current **turn**. The agent's text is streamed into it (`stream`), split into sentences and added to the channel's batch as it completes, whatever the channel's policy (a turn is many messages). A new turn (`turn_start`) drops what is left of the previous one: its unread sentences, and its item if it is being read ("one message, always the last"). Text that arrives late from an earlier turn is dropped (see `t`). Decisions (`ask`) are read before the other channels as soon as the item playing ends (the batch reading now waits), and play an earcon.

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

### Settings

| key | value |
|---|---|
| `mute_level` | `0` (default), `1`: agent text is not spoken (what is queued and playing is dropped), earcons still play; `2`: earcons are silent too. Text spoken without the extension (core `speak`, channel `speak`) is not affected |
| `verbosity` | `"everything"` (default): text, decisions, tool announcements and hints; `"medium"`: no tool announcements or hints; `"quiet"`: decisions only |
| `minqueue` | `0` to `10` (default 1): a turn's sentences are held until this many are waiting, the turn ends, a tool runs or a decision arrives; `0` and `1` read at once |
| `summaries` | `{enabled, command, model, timeout, settle_ms, style, prompt}`: see below. `set` merges the fields given; `get` returns them all |

**Summaries** (off by default). The turn's text is recorded instead of read; when the turn ends and no text came for `settle_ms` (0 to 5000, default 600), a headless agent writes a spoken recap of it: `command` `"claude"` (`claude -p`, tools and settings off) or `"codex"` (`codex exec`, read-only), `model` (default `"haiku"`), `style` `"tidy"`, `"natural"` (default) or `"brief"`, or a custom `prompt`. The command is found on `PATH` only and runs in the user's home folder with no window; past `timeout` seconds (15 to 300, default 60) it is killed with its child processes. A turn shorter than 280 characters is read as it is. A summary that fails or comes back empty falls back to the turn's text. A decision waits for the recap of the text before it (read first), at most `timeout` + 5 s. Recaps are read in the order the turns ended, and one still out after twice `timeout` is read as plain text. A new turn, an answer or `stop` drops the recaps of the channel still out. A runtime built without the summarizer answers `enabled: true` with `E_UNSUPPORTED`.

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

## Extensions (reserved)

Spec section 4.4. A client asks for it in `hello.extensions`; this runtime does not offer it yet, so its messages return `E_UNSUPPORTED`.

| extension | adds |
|---|---|
| `system` | `set audio_mode`, `set duck_level`, `set hotkeys`, `get settings_url` |
