<div align="center">
  <img src="assets/sonara-logo-minimal-02.png" alt="Sonara logo" width="128">

  # Sonara

  **Eyes-free speech for [Claude Code](https://claude.ai/code) on Windows.**<br>
  An accessibility tool for blind and low-vision developers: hear every reply, answer every prompt, keep the screen off.

  [![CI](https://github.com/Maxaubert/Sonara/actions/workflows/ci.yml/badge.svg)](https://github.com/Maxaubert/Sonara/actions/workflows/ci.yml)
  [![Release](https://img.shields.io/github/v/release/Maxaubert/Sonara?color=5B4BDB)](https://github.com/Maxaubert/Sonara/releases)
  [![License: MIT](https://img.shields.io/badge/license-MIT-5B4BDB)](LICENSE)
  ![Windows 10/11](https://img.shields.io/badge/Windows_10%2F11-0078D6?logo=windows&logoColor=white)
  ![Python 3.9+](https://img.shields.io/badge/Python_3.9%2B-3776AB?logo=python&logoColor=white)

</div>

---

Sonara is a Claude Code plugin that reads Claude's output aloud: prose, plans, multiple-choice
questions and permission prompts. A distinct sound plays the moment a decision needs you, you
answer by number, and global hotkeys control the voice from any window.

> [!NOTE]
> This is the Windows line of Sonara, forked from [nimkimi/sonari](https://github.com/nimkimi/sonari)
> and developed independently. It runs on Windows only.

## Features

- **One message, always the last.** Sonara reads the latest reply. Press **Ctrl+Alt+Up** and it
  starts that reply over; nothing older gets in the way.
- **Decision earcons.** A short sound the instant a question or permission prompt appears, in
  every session, even one that is not speaking.
- **Answer by number.** Questions and permission prompts are read with their options; press the
  option's number in Claude Code. No key injection.
- **Global hotkeys.** Restart, flush, mute and switch sessions without leaving your editor.
- **Natural voices.** Optional Kokoro neural voices run fully offline; the built-in Windows voices
  work out of the box.
- **Spoken summaries.** Optionally hear a short recap of each reply instead of the whole text,
  written by Claude (Haiku) or Codex.
- **Several sessions.** Each Claude Code session gets its own queue, name, mute and voice. Switch
  between them by hotkey.
- **Plays nicely with other audio.** Lower (duck) or pause other apps while Sonara speaks, and
  restore them afterwards.

## Requirements

- Windows 10 or 11.
- Claude Code with plugin support.
- Python 3.9 or newer is used when present. If none is found, `/sonara:install` provisions a
  private CPython 3.12 with [uv](https://github.com/astral-sh/uv); the Microsoft Store
  `python` stub is never used.

## Install

1. Add the marketplace in Claude Code:
   ```
   /plugin marketplace add Maxaubert/Sonara
   ```
2. Install the plugin (the marketplace and the plugin are both named `sonara`):
   ```
   /plugin install sonara@sonara
   ```
3. Run the one-time setup:
   ```
   /sonara:install
   ```
   It finds or provisions Python, installs the Windows speech engine (PyWinRT), copies the
   runtime to `~/.sonara/app` (so plugin updates never pull files out from under a running
   daemon), registers autostart as a per-user scheduled task, wires up the hooks, and puts a
   `sonara` command in `~/.local/bin`. If that folder is not on your PATH, install says so and
   how to add it. Each step is printed. The first run can take a couple of minutes.
4. Start a new Claude Code session, then run `/sonara:doctor`. No row should say `FAIL`
   (`warn` rows are advice, such as the AltGr warning below).

After a plugin update Sonara says once: "Sonara was updated. Run /sonara:install to apply."
Running it refreshes the copy in `~/.sonara/app`.

## How it reads

- **The latest reply, as it streams.** Sonara turns Markdown into speakable sentences (code
  blocks are summarised, not spelled out) and reads them in order. A question, plan or
  permission prompt is spoken in its natural place, after the text that explains it.
- **The alert is instant, the words wait their turn.** When a question or permission prompt
  appears, its earcon plays at once; its spoken text follows the sentences before it. A plan
  has no earcon; it is read in its place.
- **Other earcons** come from the daemon: `turn_done` when a reply ends, `nav` and `nav_edge`
  for a restart or flush (`nav_edge`: nothing to restart or flush), `session_change` when the
  voice moves to another session, `error` when speech fails, and `summary_failed` when a
  summary-mode turn had no text to read.
- **A new prompt starts fresh.** When you send the next prompt, the previous reply is dropped
  and Sonara follows the new one.
- **Restart, never rewind.** Ctrl+Alt+Up restarts the latest reply from the top (in summary
  mode it re-reads the last summary). There is no stepping back through older replies.
- **Several sessions.** The session you last prompted owns the voice. Other sessions still play
  their earcons, and their latest reply waits. Ctrl+Alt+P moves the voice to the next session
  and says "Session changed: &lt;folder&gt;"; an unread reply resumes, a read one starts over.

### Answering prompts

When a question, permission prompt or plan appears, press the option's **number (1 to 9)** in
Claude Code, or `Esc` to cancel. For a multi-select question, press each option's number (or
`Space` on the highlighted item), then `Enter`. With more than nine options, use the arrow keys
and `Enter` for the tenth and later. Sonara speaks these hints when they apply.

## Hotkeys

The default chord is **Ctrl+Alt**. Rebind any action on the settings page's Hotkeys tab or in
`~/.sonara/keymap.json`. A hotkey must include Ctrl, Alt or Win.

| Hotkey | Action |
|---|---|
| Ctrl+Alt+Up | Restart the latest reply from the top (summary mode: re-read the last summary) |
| Ctrl+Alt+Down | Flush: silence everything queued in every session and go quiet. Ctrl+Alt+Up brings the reply back |
| Ctrl+Alt+M | Mute cycle: unmuted, muted (speech), super muted (speech and earcons) |
| Ctrl+Alt+P | Move the voice to the next session |

`pause`, `faster` and `slower` are also actions; they ship unbound. Stop, skip and repeat are
CLI commands (below).

<details>
<summary>European keyboards (AltGr), and which keys to avoid</summary>

Windows sends AltGr as Ctrl+Alt, so on keyboard layouts with AltGr characters (German,
Norwegian, Polish and others) a Ctrl+Alt hotkey takes that character away: Ctrl+Alt+M bound to
mute stops AltGr+M from typing µ. `sonara doctor` names each clashing hotkey in an **AltGr**
warning row, and the settings page warns next to the binding. The fix is to rebind that key
with Win, for example to Win+Alt+Home/End or Ctrl+Win+Up/Down; a Win chord never matches
AltGr. Resetting to the defaults does not help, since they use Ctrl+Alt.

Win+Alt is not the default because Windows 11 already owns Win+Alt+Up/Down (snap a window),
Win+Alt+M and Win+Alt+P, plus Win+Alt+B (HDR), Win+Alt+D (date and time), Win+Alt+H (voice
typing), Win+Alt+K (microphone mute), Win+Alt+Enter and the Game Bar chords
Win+Alt+G/R/T/PrtScn. If another app holds a chord, `sonara doctor` reports it in its hotkey
row.

The defaults are unchanged from 0.6.x. **Reset hotkeys to defaults** on the Hotkeys tab, or
`sonara keymap --reset`, puts every binding back on them.

</details>

## Settings page

Run `/sonara:settings` (or `sonara settings`) to open the settings page in your browser. It is
served by the daemon on `127.0.0.1` and protected by a token. Changes apply immediately.

| Page | What you can set |
|---|---|
| Speech | Voice (with a preview button), speaking rate, instant cues and the cue voice |
| Summary | Summary mode (Off, Tidy, Natural, Brief), the instruction for each style, the model, and the minimum queue before live reading starts |
| Audio | Speech volume (25 to 200 percent), what other apps do while Sonara speaks (Off, Duck, Pause) and the duck level |
| Sessions | A name, mute and voice per Claude Code session |
| Hotkeys | Every binding, with AltGr and conflict warnings, and a reset to defaults |
| Advanced | Summary timeout and settle time |
| System | Daemon status, restart and shut down, Kokoro status, the version |

**Audio mode.** *Duck* lowers other apps to the duck level while Sonara speaks; *Pause* pauses
media that Windows can control (music, video players) and resumes it afterwards. Either way
Sonara restores them when it stops, and on the next start if it was killed mid-sentence.

**Summary mode.** Instead of reading a whole reply, Sonara waits for it to finish and speaks a
short recap. Questions, plans and permission prompts are still read in full and every earcon
still plays. The recap comes from a separate, throwaway call that never touches your session:

- **Claude** (default, Haiku): `claude -p` with tools and settings disabled, on your Claude Code
  login.
- **Codex** (GPT-5.6 Luna or GPT-5.4 Mini): `codex exec` in a read-only sandbox, on your Codex
  login. Needs the Codex CLI on your PATH.

Each finished reply costs one small call on that account. If the call fails, times out or
returns nothing, Sonara reads the original reply instead; the `summary_failed` earcon plays only
when the turn had no text at all. See [PRIVACY.md](PRIVACY.md) for what is sent.

## Voices

**Windows voices** work out of the box through the built-in OneCore engine. Natural voices sound
better: open *Settings > Time & language > Speech > Add voices* and add an English voice.

**Kokoro neural voices** (optional) are 28 offline voices, such as `af_heart`, that sound far
more natural:

```
sonara voices install
```

This builds a private Python environment for Kokoro under `~/.sonara/venv` (about a 316 MB
download, once); after that, synthesis is fully local. Pick a voice on the settings page or with
`sonara voice af_heart`. If Kokoro cannot speak (still downloading, or a broken install), Sonara
falls back to the best Windows voice and says why once. `sonara voices uninstall` removes it
again.

**Windows voices listed but silent?** On some PCs the voices are listed while their data files
are missing, so synthesis fails with "file not found". `sonara doctor` detects this and prints
the repair: remove and re-add *English (United States)* under *Settings > Time & language >
Speech > Manage voices*, or in an elevated prompt run
`DISM /Online /Add-Capability /CapabilityName:Language.TextToSpeech~~~en-US~0.0.1.0`. Kokoro
voices are unaffected.

## Commands

| Slash command | CLI | What it does |
|---|---|---|
| `/sonara:install` | `sonara install` | One-time setup (above); re-run after an update |
| `/sonara:settings` | `sonara settings` | Open the settings page |
| `/sonara:start` | `sonara start` | Start the daemon and clear a previous shutdown |
| `/sonara:doctor` | `sonara doctor` | Run every health check |
| `/sonara:uninstall` | `sonara uninstall` | Remove Sonara (see *Uninstall*) |

CLI only: `sonara status`, `stop` (clear everything), `skip`, `repeat`, `shutdown` (stays off
until `sonara start`), `voice [name]` (no name lists voices), `voices install|uninstall`,
`rate <wpm>`, `verbosity everything|medium|quiet`, `summary on|off`, `audio-mode off|duck|pause`,
`duck-level <0-100>`, `minqueue <n>`, `keymap` (`--reset` for the defaults), `cleanup`.

**Verbosity** (CLI only): `everything` (default) reads prose, decisions and short tool
announcements such as "Running git status"; `medium` drops the tool announcements; `quiet`
reads decisions only. Earcons play at every level.

## Troubleshooting

Run `sonara doctor` first: it checks the speech engine and a real Windows voice synthesis, the
daemon, autostart, the hooks, the hotkeys, Kokoro, the summary engine and the install.

- **No speech at all.** Run `/sonara:start` (a shutdown keeps Sonara off until then), then
  `sonara doctor`. The daemon log is `~/.sonara/speechd.log`.
- **Hooks not firing.** Make sure the `sonara` plugin is enabled in `/plugin`, start a new
  session, and check the hooks row in `sonara doctor`.
- **Too fast or too slow.** Change the speaking rate on the settings page, or
  `sonara rate 180` (default 200 words per minute).
- **Too chatty.** `sonara verbosity medium`, or turn on summary mode.
- **Something is stuck.** `sonara stop` clears every queue and cuts the current sentence.
- **Disk space after upgrading.** Sonara 0.6.1 removed the Chatterbox engine. If you had it,
  `sonara doctor` lists the leftover environment and model cache (several GB) and
  `sonara cleanup` deletes them. Your own voice clips in `~/.sonara/voices/` are kept, and a
  saved Chatterbox voice now speaks as Kokoro's `af_heart`.

## Uninstall

1. Run `/sonara:uninstall` (or `sonara uninstall`). It stops the daemon, removes the autostart
   task, the hotkeys, the `sonara` launcher, the hooks it added and the runtime copy in
   `~/.sonara/app`, and keeps Sonara off. Your `config.json` and `keymap.json` stay, so a
   reinstall restores your settings.
2. Disable or remove the `sonara` plugin in `/plugin`.
3. Optional: delete the `~/.sonara` folder to remove everything else (Kokoro voices, session
   data, logs). [PRIVACY.md](PRIVACY.md) lists what is in it.

## Privacy

Sonara runs on your computer and has no servers, accounts or telemetry. The only text that
leaves your machine is a finished reply in summary mode, sent to Anthropic (`claude -p`) or
OpenAI (`codex exec`) depending on the engine you chose. Details, and every file Sonara keeps,
are in [PRIVACY.md](PRIVACY.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the workflow and [docs/architecture.md](docs/architecture.md)
for how Sonara is built. Embedding hosts talk to the daemon over the protocol in
[docs/protocol.md](docs/protocol.md). Bugs and ideas: [GitHub issues](https://github.com/Maxaubert/Sonara/issues).

## License

MIT, see [LICENSE](LICENSE).
