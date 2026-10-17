# Troubleshooting

Fix problems with Sonara: start with `/sonara:doctor`, find your symptom below, and read the logs if you still need more.

## Step 1: run the doctor

In Claude Code run `/sonara:doctor`. It installs the runtime first if it is missing, then prints one row per check:

| Tag | Meaning |
|---|---|
| `[ OK ]` | Fine |
| `[INFO]` | Worth knowing (a model still downloading, for example) |
| `[WARN]` | Worth fixing |
| `[FAIL]` | Broken install, or the runtime cannot start |

A row looks like `[ OK ] runtime: running, version 0.32.14, pid 1234`.

## Every doctor row

Rows appear in this order. Rows from `engine` to `mute level` appear only while the runtime answers.

| Row | Tag | What it says | What to do |
|---|---|---|---|
| `version` | INFO | The version of `sonara.exe` that ran | Nothing. Compare it with the release you expect after an update |
| `home` | INFO | Sonara's folder, normally `%LOCALAPPDATA%\Sonara` | Nothing. Logs and settings live here |
| `sonarad.exe` | OK / FAIL | The speaking program is present, or "missing from ..." | FAIL: run `/sonara:start` to reinstall the runtime |
| `sonara-hook.exe` | OK / FAIL | The program Claude Code's hooks call is present, or missing | FAIL: run `/sonara:start` |
| `onnxruntime.dll` | OK / WARN | The library Kokoro needs is present. WARN says "Kokoro cannot run; Windows voices only" | Run `/sonara:start`. If it stays missing, restart Claude Code so the runtime reinstalls |
| `runtime` | OK | `running` or `started now`, with version and pid | Nothing |
| `runtime` | WARN | `stopped; run /sonara:start`, or `running ... but shut down for the hooks` | Run `/sonara:start`. See [the stopped file](#the-stopped-file) |
| `runtime` | FAIL | The runtime could not start, with the reason | Read `logs\sonarad.log` and `logs\bootstrap.log`, then run `/sonara:start`. Still failing: [report a bug](#report-a-bug) |
| `engine` | OK | `<engine>, ready` | Nothing |
| `engine` | INFO | `loading its voice model`, or `downloading its voice model: N of M MB; speaking with onecore meanwhile` | Wait. A Windows voice speaks meanwhile. See [model download](#the-voice-model-download-is-stuck) |
| `engine` | WARN | `the voice model download failed and is retried later`, with the reason in brackets, or `<engine> is <status>` with a reason | Check your network, then see [model download](#the-voice-model-download-is-stuck). For an engine you added, open Settings > Engines and use **Test** |
| `voice` | INFO | The voice in use, the speed in words per minute, and the volume in percent | Wrong values? Change them on the Speech and Audio pages |
| `voices` | INFO | How many voices each engine lists, such as `28 kokoro, 3 onecore`. A note says if Windows lists no Windows voice and only Kokoro is available | Nothing. If there is no Windows voice and Kokoro is not ready, nothing can speak yet |
| `voices` | WARN | `no engine lists a voice` | Wait for the Kokoro download, or add a Windows voice (Settings > Time & language > Speech > Add voices) |
| `hotkeys` | OK | The bound keys, such as `Ctrl+Alt+Up restart` | Nothing |
| `hotkeys` | INFO | The keys, plus `(not registered now)` | Hotkeys are off until a Claude Code session is connected. Start or restart a session |
| `hotkeys` | WARN | One or more problems, separated by `;`: taken by another program, also bound to another action, types a character with AltGr, an Intel screen-rotation note, or a `keymap.json` problem | See [hotkeys](#hotkeys-do-not-work-or-clash) |
| `audio` | INFO | What happens to other apps: paused, lowered to N %, or left alone | Change it on the Audio page (Other apps: Off, Duck, Pause) |
| `summaries` | INFO | `on (<style>, <model>)` or `off: replies are read as they are` | See [Spoken summaries](summaries.md) |
| `mute level` | INFO | `0` unmuted, `1` muted (speech), `2` super muted (speech and chimes) | If you hear nothing and it is not 0, press Ctrl+Alt+M until it is |
| `hooks` | OK | The plugin's hook file is found and matches the runtime | Nothing |
| `hooks` | WARN | The hook file does not call `bin/sonara-hook-launch`, or the plugin needs a newer runtime than the one running | Start a new Claude Code session so the new runtime installs. Check that the `sonara` plugin is enabled in `/plugin` |
| `hooks` | INFO | "run /sonara:doctor in Claude Code to check the plugin's hooks" | You ran `sonara doctor` outside the plugin. Run the slash command instead |
| `python sonara` | WARN | The old Python Sonara is still set up in `~\.sonara`. Shown only then | See [the Python plugin](#upgrading-from-the-python-plugin-010-and-older) |

If the command exits with a `[FAIL]`, its exit code is 1.

## No sound

Work down the list. Stop at the first fix that works.

1. Run `/sonara:doctor`. A `[WARN]` or `[FAIL]` row says what is wrong.
2. `runtime` says stopped: run `/sonara:start`.
3. `mute level` is 1 or 2: press **Ctrl+Alt+M** to cycle back to unmuted.
4. Session muted: on the Sessions page, check the **Audio** switch of the session. See [Several sessions](sessions.md).
5. **Other sessions** is **Chime only** and you are in a background session: the reply waits. Press **Ctrl+Alt+P** to move to it, or choose **Read all** on the Speech page.
6. Summaries on and the call failing: the original reply is read instead, so you would still hear something. If you hear only a soft falling knock, the turn had no text.
7. Volume is 0: Audio page, **Volume**.
8. Nothing since you installed the plugin: restart Claude Code. The runtime installs on the first hook of a new session. Offline or blocked downloads are retried every 5 minutes, or at once by `/sonara:start`.
9. Windows voices are listed but silent: see below.

**Windows voices listed but silent.** On some PCs the voices are listed but their data files are missing, so speech fails with "file not found". Kokoro voices are not affected. Repair:

- Remove and re-add *English (United States)* under *Settings > Time & language > Speech > Manage voices*, or
- In an elevated prompt run `DISM /Online /Add-Capability /CapabilityName:Language.TextToSpeech~~~en-US~0.0.1.0`

## Wrong voice, or an odd one

| You hear | Why | Fix |
|---|---|---|
| A Windows voice, not `af_sarah` | The Kokoro model is still downloading or loading. The `engine` row says so | Wait. It switches by itself |
| A Windows voice after the download finished | The `engine` row is WARN or the `onnxruntime.dll` row is WARN | Fix that row |
| Kokoro, though you set a cloud engine | The engine failed (no key, no credit, no network) and Kokoro reads that sentence. Sonara says why once | Settings > Engines, use **Test**. The reason is shown there and in `sonara engines list` |
| Kokoro, while muted | Added engines get nothing while you are muted | Unmute |
| Not the voice you picked | While an added engine is in use, the voice on the Speech page is the one you hear | Pick the voice on the Speech page |
| Too fast or slow | Rate is 100 to 400 words per minute, default 250 | Change **Speaking rate**, or bind the `faster` and `slower` hotkeys. Cloud engines follow the rate only within their own range |

More on engines: [Voices and engines](voices-and-engines.md).

## Hotkeys do not work or clash

Look at the `hotkeys` row of `/sonara:doctor` first.

| Row says | Meaning | Fix |
|---|---|---|
| `... is taken by another program` | Another app holds that chord | Rebind it on the Hotkeys page, or free it in the other app |
| `... is also bound to <action>` | Two Sonara actions share one chord | Rebind one of them |
| `types '<character>' with AltGr on this keyboard` | On layouts with AltGr (German, Norwegian, Polish and others), Windows sends AltGr as Ctrl+Alt. The hotkey takes that character away | Rebind that key with Win, for example Win+Alt+Home or Ctrl+Win+Up. Resetting does not help, since the defaults use Ctrl+Alt |
| `Intel graphics uses Ctrl+Alt+arrows to rotate the screen` | An Intel driver owns the arrow chords | Turn its hotkeys off in Intel Graphics Command Center, or rebind with Win |
| `keymap.json: ...` | A problem in your saved bindings | Fix it on the Hotkeys page, or **Reset hotkeys to defaults** |
| `(not registered now)` | No Claude Code session is connected | Start or restart a session |

Why not Win+Alt as the default: Windows 11 already owns Win+Alt+Up, Down, M and P, plus several others (Game Bar, HDR, voice typing). Full key list: [Hotkeys](hotkeys.md).

## The voice model download is stuck

The Kokoro model is about 350 MB. It downloads once into `%LOCALAPPDATA%\Sonara\models\kokoro\v1.0\`.

1. Run `/sonara:doctor` twice, a minute apart. If the `engine` row shows `N of M MB` and N grows, it is working. Just wait.
2. Row says the download failed and is retried later: Sonara retries with a growing delay, and picks up where it stopped (an unfinished file ends in `.part`). Check your network, proxy and disk space.
3. Offline: the install hooks stay silent and try again every 5 minutes. `/sonara:start` tries at once.
4. Each file is checked against pinned SHA-256 values. A file that fails the check is not used.
5. Last resort: run `sonara stop` (or close Claude Code), delete the `*.part` files in that folder, then run `/sonara:start`. The download starts over.

A Windows voice speaks the whole time.

## Other apps stay quieter or paused

Sonara pauses or lowers other apps while it speaks, and puts them back afterwards. It also puts them back on its next start if it was killed mid-sentence.

| Symptom | Fix |
|---|---|
| Music pauses "at random" | Background sessions speak too. In audio mode **Pause** (the default) or **Duck**, any reading pauses or lowers other apps. Choose **Off** on the Audio page if you do not want that |
| An app stayed quiet or paused after Sonara stopped | Run `/sonara:start`. Starting sweeps up anything left behind |
| An app stays paused on every message | Check `sonarad.log` for `media pause apps=...` lines to see which app and why. See the next section |

The Audio page options: **Other apps**: Off, Duck, Pause. **Duck to**: 0 to 100 percent, default 30.

## Logs

All logs are in `%LOCALAPPDATA%\Sonara\logs\`.

| File | Written by | Holds |
|---|---|---|
| `sonarad.log` | The runtime | Reading, decisions, hotkeys, other apps' audio, errors |
| `hook.log` | Each Claude Code hook | What each hook received and sent |
| `bootstrap.log` | The installer | The runtime download and install |

Older parts are kept as `sonarad.1.log`, `sonarad.2.log` and so on. A file rolls over at about 1 MB. All logs together stay under 10 MB, and the oldest go first.

**The Troubleshooting log setting.** The System page has a switch for the extra detail (key `debug_log`; it is on by default for now). With it on, `sonarad.log` also shows what came in, what Sonara decided and why, and the exact text read. That text comes from your sessions. Turn it off to keep text out of the logs. What each file holds: [PRIVACY.md](../../PRIVACY.md).

**How to read `sonarad.log`.** Each line starts with a UTC time. Useful lines:

| Line contains | Meaning |
|---|---|
| `read start item=12 session=<name> chunks=3` | Sonara began reading item 12 of a session |
| `read end item=12 finished` | It finished |
| `media pause apps=spotify (reason: reading item=12 session=<name>)` | It paused an app because of that reading. The item number ties it to `read start` |
| `media resume apps=... (reason: idle)` | It resumed the app when it went quiet |
| `duck apps=... level=30`, `restore apps=...` | Ducking and restoring |
| `reader paused`, `reader resumed` | Sonara's own speech paused and resumed |
| `hotkey <action>` | A hotkey fired, for example `hotkey mute level=1` |
| `in {...}` | What came in (log on) |
| `agent ... speak kind=question ...`, `agent ... permission: not spoken: ...` | What Sonara decided and why |
| `read text item=12 kind=prose from=turn_end text="..."` | The exact text read |
| `drop ... reason=turn_start` | Text dropped unread, and why |

**Something was read wrongly or skipped.** Find the item in `sonarad.log` (search for the first words you heard). The `in`, `agent` and `drop` lines around it tell you which part decided.

## The stopped file

`sonara stop` and `/sonara:uninstall` create an empty file named `stopped` in `%LOCALAPPDATA%\Sonara`. While it exists:

- The hooks do nothing and do not start the runtime.
- `/sonara:doctor` shows `runtime: stopped; run /sonara:start` (or "running ... but shut down for the hooks").

Run `/sonara:start` to delete it and start Sonara again.

## Report a bug

Open an issue: <https://github.com/Maxaubert/Sonara/issues>.

Include:

1. The full output of `/sonara:doctor`.
2. What you did, what you expected and what happened.
3. Your Windows version and keyboard layout, if it is about hotkeys.
4. The relevant lines of `sonarad.log` and `hook.log`, from a few seconds before the problem.

Check the lines before you paste them. With the Troubleshooting log on, they hold text from your sessions. Remove anything private, or turn the log off and repeat the problem.

## Upgrading from the Python plugin (0.10 and older)

The first start of the new runtime imports your settings from `~\.sonara` when `%LOCALAPPDATA%\Sonara` has no `config.json` yet. It imports voice, speed, volume, audio mode, summaries, hotkeys, session names and your own chimes. It never changes `~\.sonara`.

The old Python daemon is not stopped for you. If `/sonara:doctor` shows a `python sonara` warning, remove it once from Git Bash, best before updating the plugin:

```
PYTHONPATH=~/.sonara/app python -m sonara.cli uninstall
```

Otherwise two programs hold the hotkeys or speak twice.

## See also

- [Getting started](getting-started.md)
- [Hotkeys](hotkeys.md)
- [Settings](settings.md)
- [Voices and engines](voices-and-engines.md)
- [FAQ](faq.md)
- [PRIVACY.md](../../PRIVACY.md): every file Sonara keeps
