# Sonara privacy policy

_Last updated: 2026-10-01_

Sonara is a Windows accessibility plugin for [Claude Code](https://claude.ai/code) that reads
Claude Code's output aloud. This page says exactly what it does with your data, what it keeps on
your computer and what, if anything, leaves it.

## The short version

- Sonara runs on your computer. It has no servers, accounts, telemetry, analytics or crash
  reporting.
- With summary mode **off** (the default), nothing Sonara reads ever leaves your machine.
- With summary mode **on**, the text of each finished Claude reply is sent to make a short spoken
  recap: to **Anthropic** through `claude -p`, or to **OpenAI** through `codex exec`, depending on
  the summary engine you chose. Nothing else leaves the machine.
- Sonara keeps a few small files in `~/.sonara`, listed below. Some of them hold session text.

## What Sonara processes

Claude Code hands Sonara text through plugin hooks: assistant prose, question options, plan text,
permission-prompt descriptions and short tool names. Sonara turns it into sentences and speaks them
with the Windows speech engine or the Kokoro engine, both running locally. Its parts talk to each
other over a loopback connection (`127.0.0.1`) protected by a token stored in your profile. The
settings page is served on the same loopback address and needs that token too.

The reading history Sonara uses for restart and repeat is held in memory and is gone when the
daemon stops.

## Summary mode (opt-in)

When summary mode is on, Sonara waits for each reply to finish and starts a separate, throwaway
process with that reply's text (and the summary instruction) on its standard input:

- **Claude engine** (default): `claude -p`, with tools and settings disabled. It runs on your own
  Claude Code login, so the text goes to **Anthropic** under the same terms as your Claude Code
  session.
- **Codex engine**: `codex exec`, in a read-only sandbox with your MCP servers and memories
  turned off for the call. It runs on your own Codex login, so the text goes to **OpenAI** under
  your Codex terms.

The recap that comes back is spoken, and the latest one per session is stored locally (see
`session_digests.json`). Sonara itself operates no service and receives nothing.

## What Sonara stores on your computer

Everything lives under `~/.sonara` (`C:\Users\<you>\.sonara`). None of it is sent anywhere.

**Settings and install**

| File | What it holds |
|---|---|
| `config.json` | The settings you changed (voice, rate, volume, audio mode, summary options and your own summary instructions, mute state) |
| `keymap.json` | Your hotkey bindings |
| `install.json` | Paths of the Python interpreter, the plugin and the app copy, plus their versions |
| `python.path`, `pythonw.path` | The Python interpreter the hooks and the daemon use |
| `app/` | The copy of Sonara's own code that the daemon runs |

**Session data** (this is where session content is kept)

| File | What it holds |
|---|---|
| `sessions.json` | Each Claude Code session id and the name of its working folder (up to 200 sessions) |
| `session_prefs.json` | The name, mute and voice you gave a session on the Sessions page |
| `session_seen.json` | When each session was last active |
| `session_digests.json` | **The text of each session's latest spoken digest** (up to 4,000 characters each, 200 sessions), so a restarted daemon can still read a session's last message |

**Runtime state**

| File | What it holds |
|---|---|
| `daemon.lock` | The daemon's local port, process id and access token (removed when the daemon stops) |
| `webui.token` | The token for the settings page and the daemon, kept across restarts |
| `daemon.singleton` | The running daemon's process id |
| `stopped` | Present while you have shut Sonara down |
| `duck_state.json` | Which apps Sonara lowered and their original volume, so they are restored after a crash |
| `pause_state.json` | Which media apps Sonara paused, so they are resumed after a crash |
| `hotkeys.state.json` | Hotkey conflicts and whether the daemon runs elevated, for `sonara doctor` |
| `no_hotkeys` | Present only if you created it to turn global hotkeys off |
| `previews/` | Short voice samples for the settings page ("Hi! This is the ... voice.") |

**Logs**

| File | What it holds |
|---|---|
| `speechd.log`, `speechd.old.log` | Daemon startup, warnings and errors. It can contain short snippets of session text: the first 120 characters of each spoken summary, the start of a sentence dropped while muted, a summary engine's error output, and text inside an error trace |
| `faulthandler.log`, `faulthandler.prev.log` | Thread stacks if the daemon crashes natively |

**Voices and tools** (only when you installed them)

| Folder | What it holds |
|---|---|
| `venv/`, `kokoro/` | The Kokoro neural voice environment and model |
| `tools/` | `uv.exe`, used to set up Python and Kokoro |
| `chatterbox-venv/`, `chatterbox/` | The environment and model cache of the removed Chatterbox engine, left on upgraded installs until you run `sonara cleanup` |
| `voices/chatterbox/` | Voice clips you recorded for the removed Chatterbox engine. Sonara never deletes them |

Outside `~/.sonara`, setup adds a per-user Task Scheduler task (autostart), a `sonara.cmd`
launcher in `~/.local/bin`, and, when needed, Sonara's hooks in `~/.claude/settings.json`. If it
provisioned Python, that Python lives in uv's own folder.

## Downloads

Setup and optional voices download software, never your data: PyWinRT and the Kokoro packages
from PyPI, `uv` and the Kokoro model from GitHub, and (only when no Python is found) a CPython build through uv. After
that, speech synthesis is fully local.

## Diagnostic capture (off by default)

If you set the `SONARA_CAPTURE` environment variable to a folder, the hook writes every raw hook
payload it receives, including session content, into that folder on your computer, for
troubleshooting. It is off unless you set it. Delete the folder to remove what it captured.

## Removing your data

Run `sonara uninstall` (or `/sonara:uninstall`) to remove the autostart task, launcher, hooks,
app copy, daemon logs (`faulthandler.prev.log` stays) and lock files, then delete the `~/.sonara` folder to remove everything else,
including session data and your settings.

## Changes to this policy

Changes are committed to this file in the repository, with the "Last updated" date revised.

## Contact

Questions about privacy: open an issue at <https://github.com/Maxaubert/Sonara/issues>.
