# Sonara architecture

How Sonara is put together, for contributors. Current as of 0.8.2 (2026-10-01). User-facing
behaviour is in the [README](../README.md); the wire contract for embedding hosts is
[protocol.md](protocol.md).

## Big picture

Sonara is two kinds of process:

- **Hook processes**, short-lived: Claude Code runs one per hook event. Each translates the event
  into protocol messages, sends them to the daemon and exits.
- **The daemon**, long-lived, one per Windows user: it owns the speech queue, the voice, the
  hotkeys and the settings page. A per-user Task Scheduler task starts a supervisor loop at logon
  that keeps the daemon running; a hook also starts it on demand.

The daemon runs the copy of the package in `~/.sonara/app`, not the plugin's files, so plugin
updates never change code under a running daemon. `/sonara:install` refreshes that copy.

## Data flow

```
Claude Code hook event
  -> hooks/hooks.json
  -> bin/sonara-hook-run          picks the interpreter (python.path, PATH python, py -3)
  -> bin/sonara-hook              reads the event JSON on stdin
  -> hooks_entry.py               pure: event -> list of protocol messages
  -> client.py                    send_many: token line + JSON lines on one TCP connection
                                  (lifecycle.ensure_running starts the daemon if needed)
  -> daemon/server.py             token check, one handler thread per connection
  -> daemon/__init__.py           handle_message: table dispatch under the daemon lock
  -> daemon/ingest.py             prose -> assembler.py + cleaner.py -> SpeechItems;
                                  decisions -> decision_text.py; earcons; session lifecycle
  -> router.py / channel.py       one SessionChannel per session; the router picks the reader
  -> daemon/playback.py           the speak loop: next item, mute/pause rules, audio mode
  -> speaker.py                   synthesis + playback, cancel epochs, earcons
  -> platform/windows/tts.py      WinRT OneCore or Kokoro synthesis, winsound playback
```

Other producers enter at `handle_message` the same way: the CLI (`cli.py` through `client.py`),
global hotkeys (`daemon/hotkeys.py`), the settings page (`webui.py`) and embedding hosts
(SPEAK, SUBSCRIBE).

**Summary mode** branches at ingest: prose is recorded to history but not queued. When the turn
settles, `daemon/summary/pipeline.py` runs `summarizer.py` (`claude -p` or `codex exec`) on a
worker thread, and `daemon/summary/reorder.py` releases digests in turn-finish order. A question
waits behind the digest of the text that leads into it.

**One message, always the last.** A channel holds only its session's current turn; a new prompt
(FLUSH) wipes it. Spoken items are not discarded, a cursor moves over them, so restart (NAV first,
`nav_start`, default Win+Alt+Home) replays the turn from its start and a session switch can resume
or replay it. Nothing may silently drop the latest turn.

## Threads and the lock contract

The daemon has one lock, `SpeechDaemon._lock` (a `threading.Lock`). **All daemon state is
guarded by it**: router and channels, history, sessions, the summary pipeline, cues, shared
state and the per-session registry.

| Thread | What it does | Lock |
|---|---|---|
| main | `run()`: binds the socket, starts the others, waits | |
| accept | `server.accept_loop`, at most 32 request connections | |
| connection (one each) | reads lines, `handle_message_guarded` | takes the lock per message |
| SUBSCRIBE connection | writes queued state events to its socket | never writes under the lock |
| speak loop | `playback.SpeakLoop.run` | takes the lock to read state, speaks off-lock |
| synthesis | `speaker.py` renders an utterance ahead of playback | off-lock |
| hotkey listener | `platform/windows/hotkeys.py` message pump | puts fires on a queue |
| hotkey worker | applies a fire like a socket message | takes the lock |
| settings page | `webui.py` ThreadingHTTPServer | `_dispatch` takes the lock |
| summary timers and workers | settle timer, hold cap, watchdog, summarizer call | take the lock themselves; the summarizer call runs off-lock |
| Kokoro download, warm-up, previews | background work | off-lock |

Rules:

- Message handlers run with the lock held. Code that relies on that calls
  `core.assert_lock_held()`; set `SONARA_DEBUG_LOCKS=1` to make it raise (CI does).
- Never block under the lock: no synthesis, no subprocess, no socket write. Collect what you
  need under the lock, release it, then do the slow part.
- Feature modules that keep daemon state by reference (playback, cues, summary pipeline, audio)
  require that the daemon never rebinds those attributes. Mutate in place.
- `_publish_state()` runs after every handled message and around every utterance, with the
  lock held; it only queues events.
- `AudioControl` (`daemon/audio.py`) has one inner lock of its own around engage and restore,
  so a PAUSE's restore cannot land between an engage's cancel-epoch check and its duck or
  pause (F3). Order: daemon lock, then the audio lock; never the reverse.

## The daemon package

`src/sonara/daemon/`:

| Module | Responsibility |
|---|---|
| `__init__.py` | `SpeechDaemon` facade: wiring, the lock, wake and paused events, mute level, item ids, heard-markers, `_enqueue`, `_replay`, `note_spoken`, `handle_message`, `run`, `stop`; owns PING, SHUTDOWN, SUBSCRIBE |
| `core.py` | `add_handlers` (one owner per message type), `SessionRegistry`, `SharedState`, `assert_lock_held` |
| `ingest.py` | Hook traffic: PROSE, CHOICE, PLAN, PERMISSION, TOOL, EARCON, FLUSH, CHOICE_ANSWERED, session lifecycle, SPEAK; owns the prose assemblers |
| `controls.py` | PAUSE, MUTE, SKIP, STOP, NAV (restart), REPEAT, NEXT_SESSION, FLUSH_SESSION |
| `settings.py` | SET_RATE, SET_VOICE, SET_VERBOSITY, SET_MINQUEUE, SET_SUMMARY_MODE, SET_SESSION_PREF, STATUS; `set_config_value`, `set_summary_prompt` |
| `audio.py` | SET_AUDIO_MODE, SET_DUCK_LEVEL, SET_VOLUME; duck or pause other apps and restore them |
| `hotkeys.py` | Start, stop and reload the listener, debounce toggles, RELOAD_KEYMAP |
| `playback.py` | The speak loop and the deferred session-change alert |
| `cues.py` | Spoken control cues on the CONTROL channel, the fast cue voice, Kokoro notices |
| `summary/pipeline.py`, `summary/reorder.py` | Summary mode and digest ordering |
| `state_stream.py` | The state snapshot for STATUS and SUBSCRIBE events |
| `server.py` | TCP accept loop, token handshake, connection caps |
| `previews.py` | Settings-page voice previews |
| `rehydrate.py` | Re-seeds recent sessions' channels from persisted digests at startup |
| `decision_text.py` | Spoken text for questions, plans and permissions |
| `setup_health.py` | The "run /sonara:install" cue when not installed or out of date |
| `tokens.py` | The persistent access token |
| `startup.py` | `main()`: process setup, single-instance guard, building the daemon |

Handler modules (`ingest`, `controls`, `settings`) get the daemon and read its attributes at
call time, so tests can replace one. Every per-session dict or set is registered with the
`SessionRegistry`; ending a session calls `forget_session(sid)` once, so new per-session state
must be registered there or it leaks.

Outside the package: `router.py` and `channel.py` (who reads, and each session's turn),
`speaker.py`, `assembler.py` and `cleaner.py` (text to spoken items; golden cases in
`tests/fixtures/text_rules/`), `history.py` (in memory), `summarizer.py`, `kokoro.py` and
`kokoro_provision.py`, `webui.py` with `settings.html`, `cli.py`, and `install/` (install,
uninstall, doctor, cleanup, voices, the app copy and the hooks in `~/.claude/settings.json`).

## Persisted state

Every path comes from `paths.py` (the test suite redirects it per test). The full list with what
each file holds is in [PRIVACY.md](../PRIVACY.md). The ones the daemon reads back:

| File | Owner | Purpose |
|---|---|---|
| `config.json` | `config.py` | User-set settings only; written atomically |
| `keymap.json` | `keymap.py` | Hotkey bindings merged over the defaults |
| `sessions.json`, `session_seen.json` | `sessions.py` | Session folder names and last activity |
| `session_prefs.json` | `session_prefs.py` | Per-session name, mute, voice |
| `session_digests.json` | `digest_store.py` | Each session's last digest, for rehydration |
| `daemon.lock`, `webui.token` | `platform/transport.py`, `daemon/tokens.py` | Port and token for clients |
| `duck_state.json`, `pause_state.json` | `platform/windows/ducking.py`, `pausing.py` | Crash recovery for other apps' audio |
| `install.json` | `install_record.py` | What was installed where |

The JSON stores share one discipline: opt-in path (tests stay in memory), best-effort atomic
writes, a cap on entries, and a missing or corrupt file means empty.

## Platform seam

The core is OS-free: `daemon/`, `install/`, `webui.py`, `cli.py` and `summarizer.py` contain no
`sys.platform` branch and no Win32 import (`tests/test_no_os_branch_in_core.py`). OS code is
reached through `sonara.platform`:

- `get_platform()` returns the backends declared in `platform/base.py` and implemented in
  `platform/windows/`: `tts`, `earcon`, `hotkey`, `supervisor` (the scheduled task, launcher,
  hooks and stray-daemon sweep), the ducker and the pauser.
- `daemon_process()` returns `platform/windows/process.py` (faulthandler, priority, VC runtime
  preload, the single-instance mutex) before any backend loads.
- `child_processes()` returns `platform/windows/child_process.py` (spawning the summarizer,
  PATHEXT lookup, killing a process tree).
- `platform/transport.py` is the OS-free loopback TCP and lockfile code.

Tests use fakes for every backend (`tests/_fakeplatform.py`, `tests/_winfakes.py`), so the suite
needs no speech engine, audio device or real hotkeys.

## Protocol

Newline-delimited JSON over loopback TCP, authenticated by a token from `~/.sonara/daemon.lock`.
Message types are in `protocol.py`. The full contract, including SPEAK, SUBSCRIBE and the host
tab, is [protocol.md](protocol.md). Changes stay additive, and the doc changes with the code.

## Config schema

`config_schema.py` holds one `Setting` per `config.json` key: the default, a validator
(`clean`), how the settings page writes it (`page="msg"` through a protocol message,
`page="config"` through `set_config_value`), an optional live-apply hook and CLI choices.
`config.DEFAULTS`, the daemon's setters, `webui.py` and the CLI all read it. `config.json`
stores only keys the user set, so a new default reaches existing installs. In a file written
before #136 (which stored every key), a value equal to a current or past default
(`LEGACY_DEFAULTS`) counts as unset.

## How to

**Add a setting**

1. Add a `Setting` to `config_schema.SCHEMA` with a default and a validator.
2. If changing it needs work in the daemon beyond storing it, either give it a protocol message
   (`page="msg"`, a `MsgType` in `protocol.py` and a handler in `daemon/settings.py` or
   `daemon/audio.py`) or a live-apply hook (`apply="<SpeechDaemon method>"`). Otherwise
   `page="config"` is enough.
3. Read it with `config_schema.get(config, key)` or `current(...)`, never a literal default.
4. Add the control to `settings.html` (and a CLI verb in `cli.py` if it belongs there).
5. Test the validator, the handler and the page round trip (`tests/test_webui.py`,
   `tests/e2e/` for UI).

**Add a message type**

1. Add the constant to `MsgType` in `protocol.py`.
2. Handle it in the feature module that owns that state and register it in its `register()`
   through `core.add_handlers` (a second owner raises). The handler runs with the lock held and
   returns a reply dict or `None`.
3. Add a producer: `hooks_entry.py`, `cli.py`, `keymap.ACTION_MESSAGES` or `webui.py`.
4. If an embedding host may use it, document it in `docs/protocol.md`. Keep it additive.

**Add a hotkey action**

1. Add `action: message` to `keymap.ACTION_MESSAGES`; the message must already be handled.
2. Leave it unbound, or add a default key to `keymap._DEFAULT_KEYS`. Check that the Win+Alt
   chord is free on Windows 11 (see the README) and run `sonara doctor`.
3. If it is a toggle, add its message type to `daemon/hotkeys.DEBOUNCED_TYPES`.
4. Add a row with `data-action="<action>"` to the Hotkeys page in `settings.html`.
