# Settings reference

Every Sonara setting: where it is on the settings page, its key in `config.json`, its default, its range and what it does.

## Open the settings page

Run `/sonara:settings`. The page opens in your browser. The runtime serves it on `127.0.0.1` only, behind a token in the link. Changes apply at once and are saved at once (a small "Saved" toast confirms). The sidebar has a search box and a **Now** card that says which engine is reading.

Pages: Speech, Engines, Summary, Audio, Sessions, Hotkeys, System.

## Where settings are stored

| File in `%LOCALAPPDATA%\Sonara` | Holds |
|---|---|
| `config.json` | Every setting in the tables below |
| `keymap.json` | Your hotkey changes. See [hotkeys.md](hotkeys.md) |
| `session_prefs.json` | Per-session name and audio on or off. See [sessions.md](sessions.md) |
| `engines.json` | The speech engines you added. See [voices-and-engines.md](voices-and-engines.md) |
| `earcons\` | Your own chime WAVs |

The **System** page shows the exact path of `config.json`. Setting `SONARA_HOME` moves the whole folder.

**`config.json` holds only the keys you set.** A key is written the first time you change it, even if you change it to the default, and not before. A new release can therefore change a default you never chose. A missing file means all defaults.

Good to know:

| Case | What happens |
|---|---|
| A value the schema refuses (out of range, wrong type) | The default applies, the value stays in the file, and the problem is logged to `logs\sonarad.log` |
| A key this release does not know | Ignored, kept in the file |
| A file that is not valid JSON | Defaults are used and a copy is saved as `config.json.bad` |
| Keys starting with `_` (for example `_migrated`) | Kept as they are |
| Editing by hand | Do it while Sonara is stopped (`sonara stop`). A running Sonara keeps its settings in memory and overwrites your edit on its next save |

## Speech page

| Label | Key | Default | Range or choices | Effect |
|---|---|---|---|---|
| Engine | `engine` | Kokoro (see note) | `kokoro`, `onecore` (Windows voices), or the id of an engine you added | The voice engine that reads. Not stored until you pick one |
| Voice | `voice` | `af_sarah` | Any voice the engine lists, or none (the engine's own voice) | The voice. A preview button speaks a sample. If the engine lacks a saved voice, the saved choice waits in `config.json` and applies once it is available |
| Rate | `rate` | 250 | 100 to 400 words per minute, step 5 | Speaking speed |
| Mute | `mute_level` | 0 | 0 Unmuted, 1 Muted, 2 Super muted | Muted: text is not spoken, chimes still play. Super muted: chimes are silent too. Set by the Ctrl+Alt+M hotkey as well |
| Detail | `verbosity` | `skip_code` | `skip_code` (Default), `read_code` (Read code), `everything` (Everything) | Default: text and decisions, code blocks dropped. Read code: also reads code blocks line by line. Everything: also announces each tool |
| Code reading speed | `code_speed` | 80 | 50 to 100 percent of Rate, step 5 | How fast code is read. Shown only when code is read |
| Line pause | `code_pause` | 500 | 0 to 2000 ms, step 100 | Silence after each code line. Shown only when code is read |
| Step through code | `code_step` | `off` | `on`, `off` | On: reading waits after the block's announcement and after each line until you press the next-section hotkey. Shown only when code is read |
| Step through questions | `question_step` | `off` | `on`, `off` | Off: every question of a set is read in turn as "Question 1 of 4". On: only question 1 is read, and you move on with Ctrl+Alt+Down. Applies to the next set |
| File paths | `paths` | `short` | `short` (Short), `last_two` (Last 2), `last_three` (Last 3), `full` (Full) | How much of a file path is read aloud. Short drops common folders (drive, `Users\<name>`, `AppData\Local` and the like) and reads at most the last three parts |
| Pronunciations | `pronunciations` | none | Up to 200 entries. A word up to 60 characters, "say it as" up to 200 | Words said your way. They are applied before every other text rule. A word added twice keeps its last entry |
| Other sessions | `background_policy` | `all` | `all` (Read all), `earcon_only` (Chime only) | Read all: every session's latest reply is read. Chime only: only the session being read is spoken. The others play their chimes and their latest reply waits |

Note on `engine`: the page lists Kokoro, Windows voices and your added engines, plus *Add new engine*. On a fresh install Kokoro is the engine when the runtime's `onnxruntime.dll` is present, which the runtime zip ships. Otherwise the Windows voices are. Until Kokoro's model has downloaded, a Windows voice reads in its place.

## Engines page

Lists the engines you added, with status and buttons to use, test, edit or remove them. Its form (Add an engine) is a setting of its own and lives in `engines.json`, not in `config.json`. See [voices-and-engines.md](voices-and-engines.md). The page is hidden when the runtime does not allow external engines.

## Summary page

The whole group is one key, `summaries`, an object. Only the fields you set are stored.

| Label | Field in `summaries` | Default | Range or choices | Effect |
|---|---|---|---|---|
| Summary | `enabled` and `style` | off, style `natural` | Off, Tidy, Natural, Brief (`tidy`, `natural`, `brief`) | Off reads replies as they are. A style reads a short recap of each finished reply instead |
| Model | `command` and `model` | `claude` and `haiku` | Haiku (fast) on `claude`, GPT-5.6 Luna (fastest) and GPT-5.4 Mini (faster) on `codex` | Which model writes the recap. Picking one also picks the command: `claude -p` or `codex exec`. Greyed out while Summary is Off |
| Prompt | `prompts` | the built-in prompt of each style | One custom instruction per style | The instruction the model gets. Shows Default or Custom. Reset goes back to the built-in one |
| Summary timeout | `timeout` | 60 | 15 to 300 seconds | How long the call may take before it is stopped. A failed or empty recap falls back to the reply's own text |
| Settle time | `settle_ms` | 600 | 0 to 5000 ms, step 100 | How long the reply must go quiet before the recap is made |

Live reading (page group "Live reading"). These apply while Summary is Off, and are greyed out while a style is on.

| Label | Key | Default | Range or choices | Effect |
|---|---|---|---|---|
| Reading | `read_mode` | `done` | `immediate` (Immediately), `queue` (Queue), `done` (When done), `final` (Final reply) | Immediately: each sentence as it arrives. Queue: sentences are held until the queue size is reached, the turn ends, a tool runs or a question arrives. When done: held until the turn ends or a question, permission or plan arrives. Final reply: only the reply after Claude's last tool is read, at the turn end; the text written between tools ("Let me look at that") is skipped. When the reply is a short closing line after a long report, the report is read too. A question still reads the text just before it, a plan reads only the plan and a permission prompt only the prompt |
| Queue size | `minqueue` | 5 | 0 to 10 | Shown only in Queue. 0 and 1 read at once |

Each finished reply costs one small call on your Claude or Codex login. See [summaries.md](summaries.md) and [PRIVACY.md](../../PRIVACY.md).

## Audio page

| Label | Key | Default | Range or choices | Effect |
|---|---|---|---|---|
| Volume | `volume` | 100 | 0 to 100 percent, step 5 | Sonara's own volume. Chimes follow it |
| Other apps | `audio_mode` | `pause` | `off` (Off), `duck` (Duck), `pause` (Pause) | What other apps do while Sonara speaks. Pause pauses media that Windows can control and resumes it. Duck lowers other apps. Off leaves them alone. They come back when Sonara stops, and on the next start if it was killed mid-sentence |
| Duck to | `duck_level` | 30 | 0 to 100 percent | The volume other apps keep while ducked. Shown only for Duck |

### Chimes and where files live

| Item | Detail |
|---|---|
| Folder | `%LOCALAPPDATA%\Sonara\earcons\` (the page shows it) |
| Your chimes | The page lists which chimes are your own |
| Names | `choice`, `permission`, `turn_done`, `session_change`, `nav`, `step`, `nav_edge`, `error`, `summary_failed`, each as `<name>.wav` |
| Format | Any WAV: 8 to 32-bit integer or float, mono or stereo, any sample rate, up to 10 seconds and 16 MB |
| Applies | The next time that chime plays, with no restart. Delete the file to get the built-in sound back |
| A file Sonara cannot play | Skipped, the built-in sound plays, and `logs\sonarad.log` says why |

## Sessions page

| Label | Key | Default | Choices | Effect |
|---|---|---|---|---|
| Announce switches | `channel_announce` | `on` | `on`, `off` | Say the folder name ("*folder*.") when the voice moves to another session. Off keeps the chime only |
| Announce voice | `announce_voice` | Same as reading voice | any installed voice of any engine | The voice the folder name is said in. A voice that is no longer installed falls back to the reading voice |

The session list (name and Audio switch per session) is stored in `session_prefs.json`, not in `config.json`. See [sessions.md](sessions.md).

## Hotkeys page

| Label | Key | Default | Choices | Effect |
|---|---|---|---|---|
| Flush skips | `flush_scope` | `session` | `session` (This session), `all` (Everything queued) | What the flush hotkey drops. This session goes on to the next session. Everything queued also drops every other session's finished replies |

The bindings themselves are in `keymap.json`. See [hotkeys.md](hotkeys.md).

## System page

| Label | Key | Default | Choices | Effect |
|---|---|---|---|---|
| Version, Uptime, Protocol | none | | read only | The running runtime |
| Config file | none | | read only | The path of `config.json` |
| Troubleshooting log | `debug_log` | `true` (on) | `true`, `false` | Records what is read, what the hooks send and why something was skipped, in `logs\sonarad.log` and `logs\hook.log`. The logs hold session text while it is on. Turn it off to keep text out |

## Every key at a glance

| Key | Type | Default | Page |
|---|---|---|---|
| `engine` | text | Kokoro (not stored) | Speech |
| `voice` | text or null | `af_sarah` | Speech |
| `rate` | 100 to 400 | 250 | Speech |
| `volume` | 0 to 100 | 100 | Audio |
| `code_speed` | 50 to 100 | 80 | Speech |
| `code_pause` | 0 to 2000 | 500 | Speech |
| `code_step` | `on`, `off` | `off` | Speech |
| `channel_announce` | `on`, `off` | `on` | Sessions |
| `announce_voice` | `null`, `{engine, voice}` | `null` | Sessions |
| `mute_level` | 0 to 2 | 0 | Speech |
| `verbosity` | `everything`, `read_code`, `skip_code` | `skip_code` | Speech |
| `read_mode` | `immediate`, `queue`, `done`, `final` | `done` | Summary |
| `flush_scope` | `session`, `all` | `session` | Hotkeys |
| `question_step` | `on`, `off` | `off` | Speech |
| `minqueue` | 0 to 10 | 5 | Summary |
| `background_policy` | `all`, `earcon_only` | `all` | Speech |
| `summaries` | object | disabled, `claude`, `haiku`, timeout 60, settle 600, style `natural` | Summary |
| `audio_mode` | `off`, `duck`, `pause` | `pause` | Audio |
| `duck_level` | 0 to 100 | 30 | Audio |
| `debug_log` | true or false | `true` | System |
| `paths` | `short`, `last_two`, `last_three`, `full` | `short` | Speech |
| `pronunciations` | list of word and say | empty | Speech |

Older names still load: the verbosity values `all` (now `everything`), and `medium` and `quiet` (now `skip_code`).

## See also

- [Getting started](getting-started.md)
- [Hotkeys](hotkeys.md)
- [Reading and code](reading-and-code.md)
- [Voices and engines](voices-and-engines.md)
- [Summaries](summaries.md)
- [Sessions](sessions.md)
- [Troubleshooting](troubleshooting.md)
- [Privacy and every file Sonara keeps](../../PRIVACY.md)
- [Protocol reference for the same keys](../protocol-v1.md)
