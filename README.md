<div align="center">
  <img src="assets/sonara-logo-minimal-02.png" alt="Sonara logo" width="128">

  # Sonara

  **A speech runtime for Windows apps.**<br>
  Offline local voices and cloud engines behind one API, with SDKs for Node, Python and React.

  [![CI](https://github.com/Maxaubert/Sonara/actions/workflows/ci.yml/badge.svg)](https://github.com/Maxaubert/Sonara/actions/workflows/ci.yml)
  [![Release](https://img.shields.io/github/v/release/Maxaubert/Sonara?color=5B4BDB)](https://github.com/Maxaubert/Sonara/releases)
  [![License: MIT](https://img.shields.io/badge/license-MIT-5B4BDB)](LICENSE)
  ![Windows 10/11](https://img.shields.io/badge/Windows_10%2F11-0078D6?logo=windows&logoColor=white)

</div>

---

Sonara is a text-to-speech runtime (sonarad.exe) that any Windows app can bundle or talk to over a
local protocol. It owns the voice: Kokoro neural voices offline, Windows voices, or cloud and local
engines you add, with queues, global hotkeys, audio ducking and several clients sharing one
instance. SDKs for Node and Python and a React player sit on top (see [Bundle Sonara in your app](docs/bundling.md)).

The first app built on it is the **Claude Code plugin** below. It reads Claude's output aloud: prose,
plans, multiple-choice questions and permission prompts. A distinct sound plays the moment a decision
needs you, you answer by number, and hotkeys control the voice from any window.

> [!NOTE]
> This is the Windows line of Sonara, forked from [nimkimi/sonari](https://github.com/nimkimi/sonari)
> and developed independently. It runs on Windows only.

[Install](#install) · [Hotkeys](#hotkeys) · [Settings](docs/user/settings.md) · [Troubleshooting](docs/user/troubleshooting.md) · [Privacy](PRIVACY.md) · [Docs](#documentation)

## Quick start

1. In Claude Code run `/plugin marketplace add Maxaubert/Sonara`, then `/plugin install sonara@sonara`.
2. Restart Claude Code. The runtime installs itself on the first hook.
3. Send a prompt. Sonara reads the reply (a Windows voice at first, Kokoro once its model has downloaded).
4. Keys: **Ctrl+Alt+Up** back or restart, **Ctrl+Alt+Down** on or flush, **Ctrl+Alt+M** mute, **Ctrl+Alt+P** next session.
5. Something wrong? Run `/sonara:doctor`.

## Features

- **One message, always the last.** Sonara reads the latest reply. Hold **Ctrl+Alt+Up** and it
  starts that reply over; nothing older gets in the way.
- **Decision earcons.** A short sound the instant a question or permission prompt appears, in
  every session, even one that is not speaking.
- **Answer by number.** Questions and permission prompts are read with their options; press the
  option's number in Claude Code. No key injection.
- **One question at a time.** When Claude asks several questions at once, Sonara can read the
  first ("Question 1 of 4") and stop, as Claude Code shows them one by one. **Ctrl+Alt+Down** and
  **Ctrl+Alt+Up** move between them.
- **Global hotkeys.** Move a sentence back or on, restart, skip, flush, mute and switch sessions
  without leaving your editor.
- **Natural voices.** Kokoro neural voices (`af_sarah` by default) run fully offline once their
  model is downloaded; the built-in Windows voices speak while it downloads.
- **Spoken summaries.** Optionally hear a short recap of each reply instead of the whole text,
  written by Claude (Haiku) or Codex.
- **Several sessions.** Each Claude Code session gets its own queue, name, mute and voice. Switch
  between them by hotkey.
- **Plays nicely with other audio.** Lower (duck) or pause other apps while Sonara speaks, and
  restore them afterwards.

## Requirements

- Windows 10 or 11 (x64).
- Claude Code with plugin support. On Windows Claude Code needs Git for Windows; Sonara's hooks
  run through its Git Bash too.
- An internet connection on first use, for the one-time downloads below. Nothing else: no
  Python, no installer, no administrator rights.

## Install

In Claude Code:

```
/plugin marketplace add Maxaubert/Sonara
/plugin install sonara@sonara
```

Then restart Claude Code. That is all: there is no setup step.

**What happens on first use.** The first hook of the new session finds that Sonara's runtime is
missing and installs it in the background, without holding up Claude Code:

1. It downloads `sonara-runtime-win-x64-<version>.zip` (about 15 MB) and `SHA256SUMS` from the
   matching [GitHub release](https://github.com/Maxaubert/Sonara/releases), checks the zip's
   SHA-256 and unpacks it into `%LOCALAPPDATA%\Sonara\runtime\<version>\`.
2. It starts the runtime (`sonarad.exe`), which arms the hotkeys and then downloads the Kokoro
   voice model (about 350 MB, once, checked against pinned hashes) into
   `%LOCALAPPDATA%\Sonara\models\`. Until it is ready Sonara speaks with a Windows voice.

Speech starts with the next event, usually a few seconds later. `/sonara:doctor` shows the
progress of the voice download. Offline, the hooks stay silent and try again every 5 minutes.

**Defaults**

| Setting | Default |
|---|---|
| Voice | `af_sarah` (Kokoro) |
| Speaking rate | 250 words per minute |
| Detail | *Default*: code blocks and tool announcements skipped (*Read code* reads code aloud, *Everything* also announces each tool) |
| When a reply is read | Once the agent is done |
| Other sessions | Read all |
| Other apps' audio | Media paused while Sonara speaks |
| Summaries | Off |

Change them on the settings page (`/sonara:settings`). Every setting is listed in
[docs/user/settings.md](docs/user/settings.md).

**Updates.** A plugin update names a new runtime release; the next hook installs it the same way,
replaces the running runtime and removes the old version. Your settings stay.

**Coming from the old Python plugin (0.10 and older)?** Your settings are imported on the first
start. See [Troubleshooting](docs/user/troubleshooting.md) for the one-time cleanup.

## How it reads

Sonara turns the latest reply into speakable sentences and reads them in order. A question, plan
or permission prompt is spoken in its place, after the text that explains it, and its earcon plays
at once. Dates, money, units, versions, file paths, web addresses and tech terms are said the way
people say them, and you can add your own words under **Pronunciations** on the settings page.

| Topic | Page |
|---|---|
| Spoken forms, tables, file paths, code blocks, step through code | [Reading and code](docs/user/reading-and-code.md) |
| Options by number, several questions at once, step through questions | [Answering prompts](docs/user/answering-prompts.md) |
| Spoken recaps instead of the whole reply | [Summaries](docs/user/summaries.md) |
| Several sessions, names, Other sessions: Chime only | [Sessions](docs/user/sessions.md) |
| The built-in chimes and your own WAV files | [Settings](docs/user/settings.md) |

## Hotkeys

The default chord is **Ctrl+Alt**. Rebind any action on the settings page's Hotkeys tab. A hotkey
must include Ctrl, Alt or Win.

| Hotkey | Tap | Hold half a second |
|---|---|---|
| Ctrl+Alt+Up | Back one section (sentence, table row, code line, question) | Read the latest reply again from the top |
| Ctrl+Alt+Down | On one section | Flush: skip the rest of the reply being read, on to the next session |
| Ctrl+Alt+M | Mute cycle: unmuted, muted (speech), super muted (speech and earcons) | |
| Ctrl+Alt+P | Move the voice to the next session | |

`pause`, `faster` and `slower` are also actions; they ship unbound. On a European keyboard
(AltGr) Ctrl+Alt can take a character away: rebind that key with Win. Details, conflicts and every
action: [docs/user/hotkeys.md](docs/user/hotkeys.md).

## Voices and engines

- **Kokoro** (default): 28 natural English voices such as `af_sarah` and `af_heart`. The model
  (about 350 MB) downloads once, is checked against pinned SHA-256 values, then runs fully offline.
- **Windows voices** (OneCore): no download. Natural ones sound better: add one under *Settings >
  Time & language > Speech > Add voices*.
- **Your own engine** (optional): OpenAI, ElevenLabs, Azure, Google, Gemini, Cartesia, Deepgram,
  a speech server on your PC, or a program of your own. Nothing is sent anywhere until you add one
  **and** choose *Use*. No model or voice is built in: Sonara lists what the provider offers now.
- If an engine fails, Kokoro reads instead and says why once. While muted, nothing is sent.

Setup, rates, send modes, keys and the CLI: [docs/user/voices-and-engines.md](docs/user/voices-and-engines.md).

## Commands

| Slash command | What it does |
|---|---|
| `/sonara:settings` | Open the settings page |
| `/sonara:doctor` | Check the runtime, the voice engine and its download, voices, hotkeys, audio mode and hooks |
| `/sonara:start` | Start Sonara (installs the runtime if needed) and clear a previous stop |
| `/sonara:uninstall` | Remove Sonara, keeping what you choose (see [Uninstall](#uninstall)) |

The slash commands run `sonara.exe` from the runtime folder through `bin/sonara`. You can also
call it from Git Bash: `bash <plugin folder>/bin/sonara <command>`.

| CLI command | What it does |
|---|---|
| `start` | Start Sonara and keep it on; clears a stop |
| `stop` | Stop Sonara; it stays off until `start` |
| `settings` | Open the settings page in the browser (starts Sonara) |
| `doctor` | Check the install and the runtime |
| `uninstall [--keep LIST]` | Stop Sonara and remove its runtime and files. `LIST` is any of `settings`, `models`, `logs` (comma-separated) or `none`; default `settings` |
| `engines ...` | Manage the speech engines you added (`sonara engines help`) |
| `version` | Print the version |

| Environment variable | Effect |
|---|---|
| `SONARA_HOME` | The home folder (default `%LOCALAPPDATA%\Sonara`) |
| `SONARA_NO_BROWSER` | Print the settings URL only, do not open a browser |
| `SONARA_RUNTIME_ARGS` | Extra arguments for `sonarad` |

## Troubleshooting

Run `/sonara:doctor` first. Each row is `OK`, `INFO` (worth knowing, such as the voice model
still downloading), `WARN` (worth fixing) or `FAIL`.

- **No speech at all:** run `/sonara:start`, then `/sonara:doctor`. `sonara stop` and
  `/sonara:uninstall` keep Sonara off until then.
- **Nothing after installing:** restart Claude Code; the runtime installs on the first hook.
- **Logs:** `%LOCALAPPDATA%\Sonara\logs\sonarad.log` (runtime), `bootstrap.log` (install),
  `hook.log` (hooks).

More fixes (music paused "at random", hooks not firing, too fast, too chatty, Windows voices
listed but silent) and the doctor rows: [docs/user/troubleshooting.md](docs/user/troubleshooting.md).

## Uninstall

1. Run `/sonara:uninstall`. Claude asks what to keep (settings, the voice model, logs), then
   Sonara stops (other apps' audio is restored first), removes its runtime
   (`%LOCALAPPDATA%\Sonara\runtime`) and everything else in `%LOCALAPPDATA%\Sonara`, and stays
   off: the hooks do nothing until `/sonara:start`.
2. Remove the plugin: `/plugin uninstall sonara@sonara`.
3. Optional: delete `%LOCALAPPDATA%\Sonara` to remove what you kept. [PRIVACY.md](PRIVACY.md)
   lists every file.

## Privacy

Sonara runs on your computer and has no servers, accounts or telemetry. The only text that
leaves your machine is:

- a finished reply in summary mode, sent to Anthropic (`claude -p`) or OpenAI (`codex exec`)
  depending on the engine you chose;
- the text read aloud, only if you add and select an external speech engine (OpenAI, ElevenLabs,
  Azure, Google, Gemini, Cartesia, Deepgram). A speech program of your own on your PC keeps it local.

Details, and every file Sonara keeps: [PRIVACY.md](PRIVACY.md).

## Use Sonara in your own app

Apps can ship the Sonara runtime and speak through it (Windows x64 only). Each release publishes
the SDKs with the same version:

```sh
pip install sonara-client                                # Python 3.9+, standard library only
npm install @sonara/client @sonara/runtime-win32-x64     # Node 18+ / Electron: client + sonarad.exe
```

Other languages take `sonara-runtime-win-x64-<version>.zip` (with `SHA256SUMS`) from
[the releases](https://github.com/Maxaubert/Sonara/releases) and speak
[protocol v1](docs/protocol-v1.md) over TCP or HTTP. The binaries are not code-signed yet, so
sign them with your app if your installer needs it. How to bundle, start and license the runtime:
[docs/bundling.md](docs/bundling.md).

## Documentation

**For users** (start at [docs/user/README.md](docs/user/README.md)):

| Page | What it covers |
|---|---|
| [Getting started](docs/user/getting-started.md) | Install, first use, first reply |
| [Reading and code](docs/user/reading-and-code.md) | How replies are spoken, tables, paths, code |
| [Answering prompts](docs/user/answering-prompts.md) | Questions, permissions, plans |
| [Hotkeys](docs/user/hotkeys.md) | Every action, rebinding, AltGr |
| [Settings](docs/user/settings.md) | Every setting, chimes, custom sounds |
| [Voices and engines](docs/user/voices-and-engines.md) | Kokoro, Windows voices, cloud and local engines |
| [Summaries](docs/user/summaries.md) | Spoken recaps |
| [Sessions](docs/user/sessions.md) | Several Claude Code sessions |
| [Troubleshooting](docs/user/troubleshooting.md) | Fixes, doctor rows, Python-plugin migration |
| [FAQ](docs/user/faq.md) | Short answers |

**For contributors and embedders:** [CONTRIBUTING.md](CONTRIBUTING.md) ·
[Architecture](docs/architecture.md) · [Protocol v1](docs/protocol-v1.md) ·
[Engine protocol](docs/protocol-v1-engines.md) · [Bundling](docs/bundling.md) ·
[Testing](docs/testing.md) · [PRIVACY.md](PRIVACY.md) · [LICENSING.md](LICENSING.md).
Bugs and ideas: [GitHub issues](https://github.com/Maxaubert/Sonara/issues).

## License

MIT, see [LICENSE](LICENSE). Apps that bundle the runtime may sell, close-source, relicense and
sign; they ship [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) ([LICENSING.md](LICENSING.md)).
How to bundle it: [docs/bundling.md](docs/bundling.md).
