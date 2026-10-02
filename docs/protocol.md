# Sonara wire protocol

This is the contract between the Sonara daemon and everything that talks to it: the Claude Code hooks, the CLI, the settings page, hotkeys and **embedding hosts** (a terminal app that shows a player for Sonara, such as PrismTerminal). An embedding host may rely on everything stated here. Source of truth in code: `src/sonara/protocol.py` (message types), `src/sonara/daemon/` (handlers), `src/sonara/client.py` (Python helpers).

## Transport

- Loopback TCP, `127.0.0.1` on a random port. The port and an auth token are in the lockfile `~/.sonara/daemon.lock` (JSON: `host`, `port`, `token`, `pid`, `http_port`). The file lives in the user's profile; the token keeps other users' processes out.
- Every connection first sends the token as one line (`<token>\n`). A wrong token closes the connection without a reply.
- After that, each message is one line of UTF-8 JSON (newline-delimited JSON): one object with a `type` field plus that type's fields.
- Several messages may be sent on one connection; the daemon applies them in order, one at a time (`client.send_many`).
- A request connection that sends nothing for 5 s is closed. The daemon serves at most 32 request connections at once; more are closed on accept. Subscribers are exempt from both limits (see SUBSCRIBE).
- Only some messages get a reply (marked **reply** below): one JSON line on the same connection. Every other message gets none, so do not wait for one.

## Versioning and compatibility

- Senders stamp `"v": 1` (`PROTOCOL_VERSION`). The field is advisory: the daemon never checks it, and messages without it are handled the same.
- The protocol only grows: new message types and new fields are added; existing ones keep their meaning. An unknown `type` is ignored (no reply), and so is an unknown field. Hosts should ignore fields they do not know in replies and events.
- A malformed message (bad JSON, wrong field types) is dropped; the connection stays open.

## Sessions

Every speech item belongs to a **session**: a Claude Code session id, or, for SPEAK, `"<source>:<tab or default>"`. Sessions have a display name (from SESSION_START's `cwd` folder, or a label), may be muted, and have a **host tab** when an embedding host started them. The foreground session (the last one to start or receive a prompt) owns the voice; other sessions are voiced when the user switches to them, or when they are authorized (Up, repeat, a turn digest, SPEAK).

## Host tab

A host that runs Claude Code inside its tabs sets an environment variable in each tab's shell:

- `SONARA_HOST_TAB=<tab id>` (generic name, preferred), or
- `PRISM_TAB_ID=<tab id>` (accepted; `SONARA_HOST_TAB` wins when both are set).

The hook then adds `"host_tab": "<tab id>"` to SET_FOREGROUND and SESSION_START. Outside a host the field is absent. The daemon remembers the tab per session (in memory, until the session ends) and reports it as `now_playing.tab` in state events and as `host_tab` in the settings page's session list.

## Messages for embedding hosts

### SPEAK (host to daemon, no reply)

Read a text aloud.

```json
{"v": 1, "type": "speak", "text": "Build passed in 42 seconds.", "source": "prism", "tab": "tab-3", "label": "Build", "interrupt": false}
```

| Field | Type | Meaning |
|---|---|---|
| `text` | string, required | The text. Markdown and symbols are cleaned for speech (the cleaner's normalization only: no sentence assembly, so a code block is read as text, not summarised); it is never summarized. |
| `source` | non-empty string, required | The host's name. |
| `tab` | string or null | The host tab the text belongs to. null means one shared queue for the host. |
| `label` | string or null | Display name for the session, used in "Session changed: <label>." announcements and the settings page. Omit it to keep the current name (a user may have renamed the session). |
| `interrupt` | bool, default false | true also stops what this session is saying right now. Other sessions are never interrupted. |

Behaviour:

- Session id is `f"{source}:{tab or 'default'}"`. It is registered on the first SPEAK and gets `host_tab = tab`.
- **Queue of one:** a SPEAK replaces that session's turn: its unread text and the texts it already read are dropped, so Up (restart) and a manual session switch replay only the latest SPEAK. It never queues behind earlier text. Text that is already playing finishes unless `interrupt` is true. A SPEAK whose text cleans to nothing just clears the session.
- The text is voiced even though the session is not the foreground Claude session. Global pause and mute still apply: SPEAK never un-pauses or un-mutes.
- A SPEAK session lives until SESSION_END or FORGET_SESSION with its id. Either one also forgets its label and other session settings, so a host should send `session_end` for a tab it closes.

Python: `client.speak(text, source, tab=None, label=None, interrupt=False)`.

### SUBSCRIBE (host to daemon; the daemon pushes events)

Keep the connection open and receive the daemon's state whenever it changes.

```json
{"v": 1, "type": "subscribe", "events": ["state"]}
```

- `events` must contain `"state"` and nothing else (the only event kind today; default `["state"]`). Any other kind is refused.
- SUBSCRIBE must be the last message on its connection; later messages on it are ignored. Send commands on separate connections.
- The first event is the current state; after that a `state` event is pushed only when the state changed. The daemon checks after every handled message and when an utterance starts or ends. Changes that arrive without a message (a background turn digest joining the queue, a config edit on the settings page) show up at the next of those checks, which can be the end of the current utterance.
- At most 4 subscribers at once. Subscribers are exempt from the 5 s read timeout and the 32-connection cap.
- A subscriber that does not read its events falls behind, gets dropped and its connection is closed. Reconnect and subscribe again; the first event brings you up to date. The connection also closes when the daemon shuts down.
- Refusal (unknown event kind, too many subscribers): one `error` event, then the connection closes.

Python: `for event in client.subscribe(): ...` (a generator; closing it closes the connection).

#### `state` event (daemon to host)

```json
{"type": "state", "seq": 17,
 "now_playing": {"session": "prism:tab-3", "tab": "tab-3", "kind": "summary", "text": "Build passed in 42 seconds."},
 "queue": 2, "paused": false, "mute_level": 0, "volume": 100, "summary_mode": true}
```

| Field | Type | Meaning |
|---|---|---|
| `seq` | int | Increases by one on every change, per daemon run. A restart starts again from 1, so a host should treat a lower seq as a fresh daemon. |
| `now_playing` | object or null | The utterance playing now; null when silent and during a "Session changed" announcement. |
| `now_playing.session` | string or null | Its session; null for a global control cue ("Paused.", "Muted."). |
| `now_playing.tab` | string or null | The session's host tab, if any. |
| `now_playing.kind` | string | `prose`, `summary`, `choice`, `plan`, `permission`, `tool_announce`, or another cue kind. |
| `now_playing.text` | string | The spoken text. |
| `queue` | int | Unread items across all sessions (control cues not counted). |
| `paused` | bool | Speech is held (PAUSE). |
| `mute_level` | 0, 1 or 2 | 0 unmuted, 1 muted (earcons still play), 2 super muted (silent). |
| `volume` | int | Speech volume percent, 25 to 200. |
| `summary_mode` | bool | Turn summaries are on. |

#### `error` event (daemon to host)

```json
{"type": "error", "error": "too many subscribers"}
```

## Hook messages (hooks to daemon, no reply)

Every hook message carries `"t"`, the hook process start time (seconds since the epoch). Each hook event is its own process on its own connection, so the previous turn's `prose` or `turn_done` can arrive after a new prompt's `flush`; the daemon drops `prose` and `turn_done` whose `t` is older than that session's last `flush` (#174). Messages without `t` are handled as before.

Sent by the legacy Python hook (`bin/sonara-hook` before 0.11; the 0.11+ plugin speaks [protocol v1](protocol-v1.md) instead) from Claude Code hook events, all messages of one event on one connection. `session` is the Claude Code session id.

| Type | Fields | Meaning |
|---|---|---|
| `prose` | `session`, `delta` (str), `index` (int), `final` (bool) | A piece of assistant text. Assembled into sentences; `index` de-duplicates redelivered deltas, `final` ends a text block. |
| `choice` | `session`, `questions` (AskUserQuestion's list) | A question with options. Preceded by an `earcon` with `kind: "choice"`. |
| `choice_answered` | `session` | The user answered the question: stale backlog is silenced. |
| `plan` | `session`, `text` | ExitPlanMode's plan. |
| `permission` | `session`, `action`, `message` | A permission prompt. Preceded by an `earcon` with `kind: "permission"`. |
| `tool_announce` | `session`, `tool`, `summary` | A tool call (spoken only at verbosity `everything`). |
| `earcon` | `kind`, optional `session` | Play a sound. `kind: "turn_done"` (with `session`) also marks the end of a turn. |
| `flush` | `session` | A new prompt: drop the session's unread speech and start its new turn. |
| `set_foreground` | `session`, `cwd`, optional `host_tab` | This session owns the voice. Sent on prompt submit and session start. |
| `session_start` | `session`, `cwd`, `plugin_version`, `plugin_root`, optional `host_tab` | A session started. |
| `session_end` | `session` | A session ended: all its state is dropped. |

## Control messages (CLI, hotkeys, settings page, hosts)

No reply unless marked. Most controls act on the engaged session (the active reader, else the foreground session).

| Type | Fields | Meaning |
|---|---|---|
| `pause` | | Toggle pause of all speech. |
| `mute` | | Cycle mute: unmuted, muted, super muted. |
| `stop` | | Silence everything queued or cooking. |
| `skip` | | Skip the current utterance. |
| `nav` | `to: "first"` | Restart the latest turn from the top (Up). Other targets are ignored. |
| `repeat` | | Re-read the last message. |
| `flush_session` | | Flush to the end: silence every session and go idle. |
| `next_session` | | Switch the reader to the next session. |
| `set_rate` | `rate` (int, 100 to 400) or `delta` (int) | Speech rate; `delta` changes it relatively and confirms aloud. |
| `set_voice` | `voice` (str) | Voice name. |
| `set_verbosity` | `verbosity`: `everything`, `medium` or `quiet` | How much is read. |
| `set_minqueue` | `minqueue` (int, 0 to 10) | Sentences to buffer before reading starts. |
| `set_audio_mode` | `mode`: `off`, `duck` or `pause` | What happens to other apps' audio during speech. |
| `set_duck_level` | `level` (int, 0 to 100) | Other apps' volume while ducked. |
| `set_volume` | `volume` (int, 25 to 200) | Speech volume percent. |
| `set_summary_mode` | `enabled` (bool) | Turn summaries on or off. |
| `set_session_pref` | `session`, `key` (`name`, `muted` or `voice`), `value` | Per-session preference. |
| `forget_session` | `session` | Drop a stale (non-foreground) session everywhere. |
| `reload_keymap` | | Re-read keymap.json and re-register hotkeys. |
| `status` | | **reply**: settings plus the state snapshot (below). |
| `ping` | | **reply**: `{"ok": true}`. |
| `shutdown` | optional `stay_down` (bool) | **reply** `{"ok": true}`, then the daemon exits. `stay_down` also keeps the hooks from restarting it. |

### STATUS reply

```json
{"verbosity": "everything", "rate": 200, "voice": "af_heart", "foreground": "5f1c...", "minqueue": 1,
 "summary_mode": true, "seq": 17, "now_playing": null, "queue": 0, "paused": false, "mute_level": 0, "volume": 100}
```

The settings fields (`verbosity`, `rate`, `voice`, `foreground`, `minqueue`) plus every field of a `state` event except `type`, with the same `seq`. A host can poll STATUS instead of subscribing.

## Example: a host session

```text
connect 127.0.0.1:<port>
> <token>
> {"v":1,"type":"subscribe","events":["state"]}
< {"type":"state","seq":4,"now_playing":null,"queue":0,"paused":false,"mute_level":0,"volume":100,"summary_mode":true}

(second connection)
> <token>
> {"v":1,"type":"speak","text":"Tests passed.","source":"prism","tab":"tab-3","label":"Tests","interrupt":false}

(first connection)
< {"type":"state","seq":5,"now_playing":null,"queue":1,...}
< {"type":"state","seq":6,"now_playing":{"session":"prism:tab-3","tab":"tab-3","kind":"summary","text":"Tests passed."},"queue":0,...}
< {"type":"state","seq":7,"now_playing":null,"queue":0,...}
```
