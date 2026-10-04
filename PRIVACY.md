# Sonara privacy policy

_Last updated: 2026-10-04 (0.18.0: the Engines section of the settings page; 0.17.0: Cartesia, Deepgram and a program of your own as external
speech engines; 0.16.0: ElevenLabs, Azure and Google as external speech engines;
0.15.0: external speech engines you add, their keys in Windows Credential Manager)_

Sonara is a Windows accessibility plugin for [Claude Code](https://claude.ai/code) that reads
Claude Code's output aloud. This page says exactly what it does with your data, what it keeps on
your computer and what, if anything, leaves it.

## The short version

- Sonara runs on your computer. It has no servers, accounts, telemetry, analytics or crash
  reporting.
- With summary mode **off** (the default), nothing Sonara reads ever leaves your machine.
- With summary mode **on**, the text of Claude's replies is sent to make a short spoken recap:
  to **Anthropic** through `claude -p`, or to **OpenAI** through `codex exec`, depending on the
  summary engine you chose. Short replies are read as they are and never sent. Nothing else
  leaves the machine.
- Sonara keeps its runtime and a few small files in `%LOCALAPPDATA%\Sonara`, listed below.
  **The troubleshooting log holds session text**: with the setting "Troubleshooting log" on
  (the default for now) it records what Sonara read aloud and what Claude Code's hooks sent it,
  which includes your prompts' context, Claude's replies, questions and their options, and tool
  inputs such as commands and file contents. It never leaves your computer, all logs together
  stay under 10 MB (the oldest lines are deleted first), and turning the setting off stops
  recording text. No other file holds session text.
- With an **external speech engine** you added and selected (opt-in; none by default), the text
  Sonara reads aloud is sent to that engine: to the cloud service you chose (OpenAI,
  ElevenLabs, Azure AI Speech, Google Cloud Text-to-Speech, Cartesia or Deepgram), or to a speech
  server or a speech program on your own PC, which keeps it local. Nothing is sent before
  you add an engine **and** select it. Its key stays in Windows Credential Manager.
- It downloads software, never your data: its runtime from Sonara's GitHub releases and the
  Kokoro voice model, once each (see Downloads).

## What Sonara processes

Claude Code hands Sonara text through plugin hooks: assistant prose, question options, plan text,
permission-prompt descriptions and short tool names. Sonara turns it into sentences and speaks them
with the Windows speech engine or the Kokoro engine, both running locally (or, if you added and
selected one, with an external speech engine: see External voices). Its parts talk to each
other over a loopback connection (`127.0.0.1`) protected by a token stored in your profile. The
settings page is served on the same loopback address and needs that token too.

The reading history Sonara uses for restart, summaries waiting to be read and each session's
turn state are held in memory and are gone when the runtime stops.

## Summary mode (opt-in)

When summary mode is on, Sonara waits for each reply to finish and starts a separate, throwaway
process with that reply's text (and the summary instruction) on its standard input. Two
exceptions: a reply shorter than 280 characters is read as it is and never sent, and when Claude
asks a question mid-reply, the text before the question is sent at that moment (even when short)
so its recap can be read before the question:

- **Claude engine** (default): `claude -p`, with tools and settings disabled. It runs on your own
  Claude Code login, so the text goes to **Anthropic** under the same terms as your Claude Code
  session.
- **Codex engine**: `codex exec`, in a read-only sandbox with your MCP servers and memories
  turned off for the call. It runs on your own Codex login, so the text goes to **OpenAI** under
  your Codex terms.

The recap that comes back is spoken and kept in memory only. Sonara itself operates no service
and receives nothing.

## External voices (opt-in)

You can add speech engines that are not part of Sonara: a cloud service (OpenAI, ElevenLabs,
Azure AI Speech, Google Cloud Text-to-Speech, Cartesia, Deepgram), a local OpenAI-compatible
server (Kokoro-FastAPI, LocalAI, Speaches, a Chatterbox server), or a speech program you installed
yourself (kind `command`, for example Piper; you add such a program yourself with `sonara engines
add <id> --kind command` or in `engines.json`, and no app or web page connected to Sonara, the
settings page included, can add or change one). Adding
one sends nothing; it is used only once you select it (*Use* under Engines on the settings page,
`sonara engines use <id>`, or `set engine` from a client).

- **What is sent.** While an external engine is selected, the text Sonara reads aloud (the same
  sentences it would speak, after its own text rules) goes to that engine, one sentence at a
  time, together with the voice, the speed and the model you set. It goes to the address you
  gave: under the provider's own terms for a cloud service, or to a program on your own PC for a
  local server (`127.0.0.1`, `localhost`), where it stays on your computer. A `command` engine
  starts the program you named, on your PC, and gives it the sentence (on its standard input or
  in a temporary file, `%TEMP%\sonara-tts-<n>.txt`, deleted after each sentence; never on its
  command line); Sonara sends nothing over the network for it, and what the program does with
  the text is up to that program. Its temporary audio file, if it writes one, is in your
  `%TEMP%` folder (`sonara-tts-<n>.wav`) and is deleted after each sentence. A voice list
  (`voices` with `refresh`, or the voice picker) asks the same address. The cloud addresses:

  | Engine kind | Text goes to | Key sent as |
  |---|---|---|
  | OpenAI (`openai-compatible`, preset `openai`) | `api.openai.com` | `Authorization` header |
  | ElevenLabs (`elevenlabs`) | `api.elevenlabs.io` | `xi-api-key` header |
  | Azure AI Speech (`azure`) | `<region>.tts.speech.microsoft.com`, or the endpoint you gave | `Ocp-Apim-Subscription-Key` header |
  | Google Cloud Text-to-Speech (`google`) | `texttospeech.googleapis.com` | `X-goog-api-key` header (with `x-goog-user-project` when you set a project) |
  | Cartesia (`cartesia`) | `api.cartesia.ai` | `Authorization` header (with the `Cartesia-Version` date) |
  | Deepgram (`deepgram`) | `api.deepgram.com` (or `api.eu.deepgram.com` when you set it) | `Authorization` header |

  Providers may keep what they receive under their terms: ElevenLabs, for example, keeps a
  history of generated speech on your account unless your plan offers zero retention (the
  engine's `enable_logging: false` option asks for it). Azure also receives Sonara's version in
  the `User-Agent` header.
- **Nothing is sent while Sonara is muted.** While Sonara is muted or super-muted (the mute
  hotkey, the settings page's mute level, or `mute` from any client), nothing goes to an external
  engine: Sonara reads with its built-in voice (Kokoro, else the Windows voices) on your PC, says
  its own short confirmations ("Muted.", "Unmuted.") with that voice too, does not ask for voice
  lists, and cuts a sentence that was on its way the moment you mute. Only two things you ask for
  on purpose still reach the engine while muted: the *Test* button (`engine_test`) and a voice
  preview. Unmuted, the next sentence goes to the engine again.
- **What is not sent.** No other text, file, setting or identifier. Short repeated phrases may be
  answered from memory instead of asked again; that memory is gone when the runtime stops.
- **Keys.** An engine's API key is stored in **Windows Credential Manager** (a generic
  credential named `sonara:<engine id>`, for your Windows user on this PC, not roaming), or read
  from an environment variable you named. It is never written to a file in
  `%LOCALAPPDATA%\Sonara`, never logged and never sent back to a client. It is sent only in the
  provider's authentication header, over HTTPS (or to a server on your own PC), never in an
  address, and only to the address it was entered for: a redirect to another address is not
  followed. Requests to a cloud service use your Windows proxy settings, if any; requests to a
  server on your own PC never go through a proxy. A key for a server on your own PC goes to
  whatever program listens on that port, so do not store one for a local server you do not
  keep running. A `command` engine that has a key gets it in its environment
  (`SONARA_ENGINE_KEY`), never as an argument, and only the program it was entered for.
- **When it fails.** If the engine cannot speak (no key, a refused key, no credit, no network, a
  server problem), Sonara reads that sentence with its built-in voice (Kokoro, else the Windows
  voices) and says once why ("OpenAI cannot be reached. Reading with the built-in voice."). The
  log line names the engine and the reason, never the text or the key.
- **The settings page.** Its Engines section adds, tests, edits and removes engines through the
  runtime's local API, like the CLI (a program on this PC it only uses, tests and removes: you add
  that yourself). A key typed there goes to the runtime once, when you save,
  and is never shown again: the field empties and no reply carries it. *Test* sends one sample
  sentence (or the text you give) to the engine.
- **Removing it.** *Remove* on the settings page, `sonara engines remove <id>` (or `engine_remove`) deletes the engine and its
  stored key. `/sonara:uninstall` deletes every `sonara:*` credential unless you keep your
  settings.

## What Sonara stores on your computer

Since 0.11 (#202) everything lives under `%LOCALAPPDATA%\Sonara`
(`C:\Users\<you>\AppData\Local\Sonara`). None of it is sent anywhere.

**The runtime**

| Path | What it holds |
|---|---|
| `runtime\<version>\` | Sonara's programs (`sonarad.exe`, `sonara-hook.exe`, `sonara.exe`), Microsoft's ONNX Runtime and the Visual C++ runtime DLLs it needs, and the licence notices, from the release zip. Only the current version is kept |
| `runtime\.bootstrap.lock` | Present while the runtime is being installed |
| `runtime\.bootstrap.failed` | The time and reason of the last failed install (a download error or a checksum mismatch), so hooks wait a few minutes before trying again |
| `models\kokoro\v1.0\` | The Kokoro voice model (`kokoro-v1.0.onnx`, `voices-v1.0.bin`; a download in progress is `*.part`) and `verified.json` (size, time and hash of the checked files) |

**Settings**

| File | What it holds |
|---|---|
| `config.json` | The settings you changed (voice, rate, volume, audio mode, mute level, verbosity, reading mode, what the flush hotkey skips, summary options and your own summary instructions, the troubleshooting log on or off), and when the settings were imported from the Python plugin. `config.json.bad` is a copy of a file that could not be read |
| `keymap.json` | Your hotkey bindings |
| `session_prefs.json` | The name, mute and voice you gave a session on the Sessions page, per Claude Code session id (the 200 most recently changed) |
| `engines.json` | The external speech engines you added: for each its id, kind, label, address, model, voice, where its key comes from (`credman`, `env:NAME` or none), for an `env:` key the address you allow it to go to (`key_origin`), and its options. Never a key. `engines.json.bad` is a copy of a file that could not be read |
| `earcons\` | Your own chimes, if you put any there: `<kind>.wav` files (for example `session_change.wav`) that Sonara plays instead of its built-in sounds. Created empty at start; Sonara only reads it |

**Runtime state**

| File | What it holds |
|---|---|
| `runtime.json` | The running runtime's process id, local ports and access token, readable only by you (removed when it stops) |
| `stopped` | Present while Sonara is stopped (`sonara stop`, `/sonara:uninstall`): the hooks then do not start or install it |
| `state\duck_state.json` | Which apps Sonara lowered and their original volume, so they are restored after a crash |
| `state\pause_state.json` | Which media apps Sonara paused, so they are resumed after a crash |

**Logs**

Everything in `logs\` together is kept at or under 10 MB: each log is split into files of about
1 MB (`sonarad.log`, then `sonarad.1.log`, `sonarad.2.log`, ... from newer to older), and when a
new line would pass 10 MB the oldest files are deleted first.

| File | What it holds |
|---|---|
| `logs\sonarad.log` (and `sonarad.<n>.log`) | When an external engine is added, removed or gets a key (its id, kind and the address's host; never the key), and when it could not speak and the built-in voice read instead (the engine, the reason, the HTTP status and the provider's message with key-like words removed; never the text). One line per start (version, process id, speech engine and whether its voice model is ready, the home folder), when the voice model becomes ready or fails, what the settings import did, settings that could not be applied, and which of your own chimes are used or could not be read. Also what Sonara did and when (UTC): each message it started and finished reading (a number, the session's name, how many sentences, what kind of text it was and which message produced it), text it dropped before reading it and why, questions and permission prompts that arrived (their kind and session), each hotkey used, the spoken confirmations ("Paused."), and which apps it paused, resumed, lowered or restored (their process names) and why. With the troubleshooting log on, also the session text: every message the hooks and other clients sent (what Claude wrote, questions with their options, permission prompts, tool names and summaries; never the access token), what Sonara decided to read or not and why, and the exact text it read aloud. With it off, none of that text: only the kinds, numbers and reasons |
| `logs\hook.log` (and `hook.<n>.log`) | One line per hook call (the Claude Code event, the session id, the tool or notification name, what was sent to the runtime and whether it arrived, and how long it took). With the troubleshooting log on, also the hook's raw input from Claude Code: your session's context such as the working folder and transcript path, Claude's messages, questions and their options, notifications, and the questions Claude asks you (any single value over 4 KB is cut short). Other tools' inputs (commands, file contents) are never kept, only their field names, and credential-looking values (API keys, tokens, passwords, `Authorization` headers) are replaced with `[redacted]` in every log line. With it off, none of that |
| `logs\sonarad.old.log` | Left by 0.13.1 and earlier; deleted first when the logs need room |
| `logs\bootstrap.log` | Each runtime download and install, with its address and result |

**The troubleshooting log setting.** On the settings page under System, "Troubleshooting log:
record what is read and what hooks send" (`debug_log` in `config.json`). It is on by default for
now, to diagnose what Sonara reads; turn it off to keep text out of the logs from then on. Lines
already written stay until they age out or you delete the `logs` folder.

**The Python plugin's folder.** Up to 0.10 Sonara kept its files in `~/.sonara`
(`C:\Users\<you>\.sonara`). The first start of 0.11 reads the settings there (`config.json`,
`keymap.json`, `session_prefs.json`) once to import them, and never changes or deletes that
folder. What the old version stored there is described in this file's history, and the old
version's own uninstall removes it.

**Outside the home: Windows Credential Manager**

| Entry | What it holds |
|---|---|
| `sonara:<engine id>` (generic credential, user `sonara`) | The API key you gave an external speech engine, for your Windows user on this PC, with the address (`scheme://host:port`) it was entered for: Sonara sends the key only there, and deletes it when the engine is changed to point elsewhere. Removed with the engine, and by `/sonara:uninstall` unless you keep your settings |

Other than those credentials, Sonara writes nothing outside `%LOCALAPPDATA%\Sonara`: the plugin
itself lives where Claude Code keeps plugins, and there is no autostart task, launcher or
settings.json change.

**Testing aid.** `sonarad --keys fake` (test runs only) keeps keys in `fake-keys.json` in the home
instead of Credential Manager. The plugin never starts it that way.

## Downloads

Sonara downloads software, never your data, over HTTPS from GitHub:

- **The runtime**, on first use and after a plugin update:
  `https://github.com/Maxaubert/Sonara/releases/download/v<version>/sonara-runtime-win-x64-<version>.zip`
  (about 15 MB) and `SHA256SUMS` of the same release. The zip is installed only when its SHA-256
  matches.
- **The Kokoro voice model**, once, by the runtime: two files (about 350 MB) from
  `https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0`, each checked
  against a SHA-256 value built into Sonara.

Like any download, these requests show GitHub your IP address. After that, speech synthesis is
fully local.

## Diagnostic capture (off by default)

If you set the `SONARA_CAPTURE` environment variable to a folder, the hook writes every raw hook
payload it receives, including session content, into that folder on your computer, for
troubleshooting. It is off unless you set it. Delete the folder to remove what it captured.

## Removing your data

Run `/sonara:uninstall`. It asks what to keep (your settings, the voice model, the logs), stops
Sonara, and removes `runtime\` and every other file in `%LOCALAPPDATA%\Sonara` except those
(and, unless you keep your settings, your external engines' keys in Credential Manager), then
writes the `stopped` file so Sonara stays off. Remove the plugin with
`/plugin uninstall sonara@sonara`, and delete `%LOCALAPPDATA%\Sonara` to remove what you kept.

## Changes to this policy

Changes are committed to this file in the repository, with the "Last updated" date revised.

## Contact

Questions about privacy: open an issue at <https://github.com/Maxaubert/Sonara/issues>.
