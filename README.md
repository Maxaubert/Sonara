<div align="center">
  <img src="assets/sonara-logo-minimal-02.png" alt="Sonara logo" width="128">

  # Sonara

  **Eyes-free speech for [Claude Code](https://claude.ai/code) on Windows.**<br>
  An accessibility tool for blind and low-vision developers: hear every reply, answer every prompt, keep the screen off.

  [![CI](https://github.com/Maxaubert/Sonara/actions/workflows/ci.yml/badge.svg)](https://github.com/Maxaubert/Sonara/actions/workflows/ci.yml)
  [![Release](https://img.shields.io/github/v/release/Maxaubert/Sonara?color=5B4BDB)](https://github.com/Maxaubert/Sonara/releases)
  [![License: MIT](https://img.shields.io/badge/license-MIT-5B4BDB)](LICENSE)
  ![Windows 10/11](https://img.shields.io/badge/Windows_10%2F11-0078D6?logo=windows&logoColor=white)

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

**Defaults** are the voice `af_sarah` at 250 words per minute, verbosity *Skip code* (code blocks
and tool announcements skipped), each answer read once the agent is done, every session read, media paused while Sonara speaks, summaries off. Change
them on the settings page (`/sonara:settings`).

**Updates.** A plugin update names a new runtime release; the next hook installs it the same way,
replaces the running runtime and removes the old version. Your settings stay.

<details>
<summary>Upgrading from the Python plugin (0.10 and older)</summary>

The first start imports your settings from `~/.sonara` (voice, speed, audio mode, summaries,
hotkeys, session names) into `%LOCALAPPDATA%\Sonara` and never changes `~/.sonara`. The old
Python daemon is not stopped for you: if `/sonara:doctor` shows a `python sonara` warning, remove
it once from Git Bash with `PYTHONPATH=~/.sonara/app python -m sonara.cli uninstall` (best before
updating the plugin), so two programs do not hold the hotkeys or speak twice.

</details>

## How it reads

- **The latest reply, as it streams.** Sonara turns Markdown into speakable sentences (code
  blocks are summarised, not spelled out) and reads them in order. A question, plan or
  permission prompt is spoken in its natural place, after the text that explains it.
- **The alert is instant, the words wait their turn.** When a question or permission prompt
  appears, its earcon plays at once; its spoken text follows the sentences before it. A plan
  has no earcon; it is read in its place.
- **Other earcons** come from the runtime: `turn_done` when a reply ends, `nav` and `nav_edge`
  for a restart or flush (`nav_edge`: nothing to restart, or nothing to flush), `session_change` when the
  voice moves to another session (followed by "Session changed: *folder*."), `error` when
  speech fails, and `summary_failed` when a summary-mode turn had no text to read. You can
  replace any of them with your own sound: see [Custom chimes](#custom-chimes).
- **A new prompt starts fresh.** When you send the next prompt, the previous reply is dropped
  and Sonara follows the new one.
- **Restart, never rewind.** Ctrl+Alt+Up restarts the latest reply from the top (in summary
  mode it re-reads the last summary). There is no stepping back through older replies.
- **Several sessions.** Every session's latest reply is read, one after another. With *Background sessions: earcons only* on the settings page, only that
  session is read; the others play their earcons and their latest reply waits. Ctrl+Alt+P moves
  the voice to the next session; an unread reply resumes, a read one starts over. Every move to
  another session, by the hotkey or because another session's reply is next, plays the
  `session_change` chime and then says "Session changed: *folder*." ("..., reading again." when
  it starts over). Give a session another name on the Sessions page.

### Answering prompts

When a question, permission prompt or plan appears, press the option's **number (1 to 9)** in
Claude Code, or `Esc` to cancel. For a multi-select question, press each option's number (or
`Space` on the highlighted item), then `Enter`. With more than nine options, use the arrow keys
and `Enter` for the tenth and later. Sonara speaks these hints when they apply.

## Hotkeys

The default chord is **Ctrl+Alt**. Rebind any action on the settings page's Hotkeys tab (it is
saved in `%LOCALAPPDATA%\Sonara\keymap.json`). A hotkey must include Ctrl, Alt or Win.

| Hotkey | Action |
|---|---|
| Ctrl+Alt+Up | Restart the latest reply from the top (summary mode: re-read the last summary) |
| Ctrl+Alt+Down | Flush: skip the reply being read: what is playing, what that session has queued and the rest of that reply still arriving (its questions are still read). *Flush skips* on the Hotkeys tab sets what else goes: *This session* (default) goes on to the next session; *Everything queued* also drops every other session's finished replies waiting to be read. A session still writing its reply keeps it either way. To silence everything, mute (Ctrl+Alt+M). Ctrl+Alt+Up replays what had arrived before the flush |
| Ctrl+Alt+M | Mute cycle: unmuted, muted (speech), super muted (speech and earcons) |
| Ctrl+Alt+P | Move the voice to the next session |

`pause`, `faster` and `slower` are also actions; they ship unbound. Each hotkey confirms itself
with a short spoken cue ("Muted.", "Rate 275.").

<details>
<summary>European keyboards (AltGr), and which keys to avoid</summary>

Windows sends AltGr as Ctrl+Alt, so on keyboard layouts with AltGr characters (German,
Norwegian, Polish and others) a Ctrl+Alt hotkey takes that character away: Ctrl+Alt+M bound to
mute stops AltGr+M from typing µ. `/sonara:doctor` names each clashing hotkey in its hotkeys
warning row, and the settings page warns next to the binding. The fix is to rebind that key
with Win, for example to Win+Alt+Home/End or Ctrl+Win+Up/Down; a Win chord never matches
AltGr. Resetting to the defaults does not help, since they use Ctrl+Alt.

Win+Alt is not the default because Windows 11 already owns Win+Alt+Up/Down (snap a window),
Win+Alt+M and Win+Alt+P, plus Win+Alt+B (HDR), Win+Alt+D (date and time), Win+Alt+H (voice
typing), Win+Alt+K (microphone mute), Win+Alt+Enter and the Game Bar chords
Win+Alt+G/R/T/PrtScn. If another app holds a chord, `/sonara:doctor` reports it in its hotkeys
row.

The defaults are unchanged from 0.6.x. **Reset hotkeys to defaults** on the Hotkeys tab puts
every binding back on them.

</details>

## Settings page

Run `/sonara:settings` to open the settings page in your browser. It is served by the runtime on
`127.0.0.1` and protected by a token. Changes apply immediately and are saved in
`%LOCALAPPDATA%\Sonara\config.json`.

| Page | What you can set |
|---|---|
| Speech | The voice engine's status (Kokoro, with Windows' voice standing in while it downloads), voice (with a preview button), speaking rate, mute level, verbosity (Everything or Skip code), background sessions |
| Summary | Summary mode (Off, Tidy, Natural, Brief), the instruction for each style, the model, and live reading: Immediately, Queue (with its queue size) or When done |
| Audio | Speech volume, what other apps do while Sonara speaks (Off, Duck, Pause), the duck level, and the folder for your own chimes |
| Sessions | A name and audio on/off per Claude Code session, and switch announcements |
| Hotkeys | Every binding, with AltGr and conflict warnings, and a reset to defaults |
| Advanced | Summary timeout and settle time |
| System | Runtime version, uptime and protocol, the settings file |

**Audio mode.** *Duck* lowers other apps to the duck level while Sonara speaks; *Pause* pauses
media that Windows can control (music, video players) and resumes it afterwards. Either way
Sonara restores them when it stops, and on the next start if it was killed mid-sentence.

### Sounds

Sonara's built-in chimes are original sounds, synthesised from scratch for it (MIT, no samples;
`packaging/sounds` rebuilds them):

| Chime | When | Sound |
| --- | --- | --- |
| `choice` | a question needs your answer | kalimba, two notes rising an octave |
| `permission` | a permission prompt | two soft rising blips |
| `turn_done` | a reply ends | kalimba arpeggio landing on the tonic |
| `session_change` | the voice moves to another session | slow, warm e-piano arpeggio |
| `nav`, `nav_edge` | a restart or flush (`nav_edge`: nothing to restart) | a light tick (the same sound) |
| `error` | speech fails | a low falling knock pair |
| `summary_failed` | a summary-mode turn had no text | a short, soft falling knock |

A WAV of your own in `%LOCALAPPDATA%\Sonara\earcons\` overrides any of them: see below.

### Custom chimes

Put a WAV file named after a chime in `%LOCALAPPDATA%\Sonara\earcons\` and Sonara plays it
instead of the built-in sound: `session_change.wav`, `turn_done.wav`, `choice.wav`,
`permission.wav`, `summary_failed.wav`, `error.wav`, `nav.wav` or `nav_edge.wav`. Any WAV works
(8 to 32-bit or float, mono or stereo, any sample rate, up to 10 seconds). It is used from the
next time that chime plays, with no restart; delete the file to get the built-in sound back. The
Audio page shows the folder and which chimes are your own. A file Sonara cannot play is skipped
(the built-in sound plays) and `logs\sonarad.log` says why. Chimes you set up in the Python
version (`~/.sonara`) are copied there when Sonara imports its settings (the runtime's first
start; from 0.11.1).

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

**Kokoro** (the default) gives 28 natural English voices such as `af_sarah` and `af_heart`. Its
model (about 350 MB) is downloaded once on first use, checked against pinned SHA-256 values,
and then runs fully offline (Microsoft's ONNX Runtime, shipped in the runtime zip). An
interrupted download resumes; a failed one is retried later. Until it is ready Sonara speaks
with a Windows voice. Pick a voice on the settings page.

**Windows voices** (OneCore) need no download. Natural ones sound better: open *Settings > Time &
language > Speech > Add voices* and add an English voice.

**Windows voices listed but silent?** On some PCs the voices are listed while their data files
are missing, so synthesis fails with "file not found". The repair: remove and re-add *English
(United States)* under *Settings > Time & language > Speech > Manage voices*, or in an elevated
prompt run `DISM /Online /Add-Capability /CapabilityName:Language.TextToSpeech~~~en-US~0.0.1.0`.
Kokoro voices are unaffected.

## Commands

| Slash command | What it does |
|---|---|
| `/sonara:settings` | Open the settings page |
| `/sonara:doctor` | Check the runtime, the voice engine and its download, voices, hotkeys, audio mode and hooks |
| `/sonara:start` | Start Sonara (installs the runtime if needed) and clear a previous stop |
| `/sonara:uninstall` | Remove Sonara, keeping what you choose (see *Uninstall*) |

They run `sonara.exe` from the runtime folder through `bin/sonara`, which you can also call from
Git Bash: `bash <plugin folder>/bin/sonara <command>`. Besides the four above it has `stop`
(Sonara stays off, and the hooks do not start it, until `start`) and `version`.

## Troubleshooting

Run `/sonara:doctor` first. Each row is `OK`, `INFO` (worth knowing, such as the voice model
still downloading), `WARN` (worth fixing) or `FAIL`.

- **No speech at all.** Run `/sonara:start` (`sonara stop` and `/sonara:uninstall` keep Sonara
  off until then), then `/sonara:doctor`. The runtime's log is
  `%LOCALAPPDATA%\Sonara\logs\sonarad.log`; the install's is `logs\bootstrap.log` there.
- **Music or video paused or quieter "at random".** In audio mode *Pause* (the default) or
  *Duck*, Sonara pauses or lowers other apps while it reads, including messages from background
  sessions. `%LOCALAPPDATA%\Sonara\logs\sonarad.log` shows each one with a UTC time:
  `read start item=12 session=<name> chunks=3`, then `media pause apps=spotify.exe (reason:
  reading item=12 session=<name>)`, `read end item=12 finished` and `media resume apps=...
  (reason: idle)`; `reader paused` and `reader resumed` mark a pause of Sonara itself; ducking
  logs `duck apps=... level=30` and `restore apps=...`, hotkeys `hotkey <action>`.
  Set audio mode to *Off* on the settings page if you do not want this.
- **It read something else, or skipped something.** With *Troubleshooting log* on (settings
  page, System; on by default for now) `sonarad.log` also shows what came in (`in {...}`), what
  was decided and why (`agent ... speak kind=question ...`, `agent ... permission: not spoken:
  ...`), the exact text read (`read text item=12 kind=prose from=turn_end text="..."`) and text
  dropped unread with the reason (`drop ... reason=turn_start`); `logs\hook.log` has what each
  Claude Code hook received and sent. The logs hold session text then; all of them together stay
  under 10 MB, oldest first out (see PRIVACY.md). Turn it off to keep text out.
- **Nothing after installing the plugin.** Restart Claude Code; the runtime installs on the first
  hook. Offline or blocked downloads are retried every 5 minutes, or at once by `/sonara:start`.
- **Hooks not firing.** Make sure the `sonara` plugin is enabled in `/plugin`, start a new
  session, and check the hooks row of `/sonara:doctor`.
- **Too fast or too slow.** Change the speaking rate on the settings page (default 250 words per
  minute), or bind the `faster` and `slower` hotkeys.
- **Too chatty.** Set verbosity to *Skip code*, turn on summary mode, or mute (the mute hotkey
  cycles muted and super muted), on the settings page.

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
leaves your machine is a finished reply in summary mode, sent to Anthropic (`claude -p`) or
OpenAI (`codex exec`) depending on the engine you chose, and, only if you add and select an
external speech engine (OpenAI, ElevenLabs, Azure, Google, Cartesia, Deepgram), the text read
aloud, sent to that provider (a speech program of your own on your PC keeps it local). Details, and every file Sonara keeps,
are in [PRIVACY.md](PRIVACY.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the workflow and [docs/architecture.md](docs/architecture.md)
for how Sonara is built. Embedding hosts talk to the runtime over the protocol in
[docs/protocol-v1.md](docs/protocol-v1.md). Bugs and ideas: [GitHub issues](https://github.com/Maxaubert/Sonara/issues).

## License

MIT, see [LICENSE](LICENSE). Apps that bundle the new runtime may sell, close-source, relicense and sign;
they ship [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) ([LICENSING.md](LICENSING.md)). How to bundle it:
[docs/bundling.md](docs/bundling.md).
