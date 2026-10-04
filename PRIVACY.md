# Sonara privacy policy

_Last updated: 2026-10-04 (0.14.1: what the flush hotkey skips is a saved setting)_

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
- It downloads software, never your data: its runtime from Sonara's GitHub releases and the
  Kokoro voice model, once each (see Downloads).

## What Sonara processes

Claude Code hands Sonara text through plugin hooks: assistant prose, question options, plan text,
permission-prompt descriptions and short tool names. Sonara turns it into sentences and speaks them
with the Windows speech engine or the Kokoro engine, both running locally. Its parts talk to each
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
| `logs\sonarad.log` (and `sonarad.<n>.log`) | One line per start (version, process id, speech engine and whether its voice model is ready, the home folder), when the voice model becomes ready or fails, what the settings import did, settings that could not be applied, and which of your own chimes are used or could not be read. Also what Sonara did and when (UTC): each message it started and finished reading (a number, the session's name, how many sentences, what kind of text it was and which message produced it), text it dropped before reading it and why, questions and permission prompts that arrived (their kind and session), each hotkey used, the spoken confirmations ("Paused."), and which apps it paused, resumed, lowered or restored (their process names) and why. With the troubleshooting log on, also the session text: every message the hooks and other clients sent (what Claude wrote, questions with their options, permission prompts, tool names and summaries; never the access token), what Sonara decided to read or not and why, and the exact text it read aloud. With it off, none of that text: only the kinds, numbers and reasons |
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

Outside `%LOCALAPPDATA%\Sonara` Sonara writes nothing: the plugin itself lives where Claude Code
keeps plugins, and there is no autostart task, launcher or settings.json change.

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
Sonara, and removes `runtime\` and every other file in `%LOCALAPPDATA%\Sonara` except those,
then writes the `stopped` file so Sonara stays off. Remove the plugin with
`/plugin uninstall sonara@sonara`, and delete `%LOCALAPPDATA%\Sonara` to remove what you kept.

## Changes to this policy

Changes are committed to this file in the repository, with the "Last updated" date revised.

## Contact

Questions about privacy: open an issue at <https://github.com/Maxaubert/Sonara/issues>.
