<div align="center">
  <img src="assets/sonara-logo-minimal-02.png" alt="Sonara" width="128">

  # Sonara

  **Eyes-free text-to-speech for [Claude Code](https://claude.ai/code) on Windows, an accessibility tool for blind and low-vision developers.**

</div>

---

> The Windows line of Sonara, forked from [nimkimi/sonari](https://github.com/nimkimi/sonari) (the macOS line). Developed and released independently.

Sonara reads Claude Code's output aloud - prose, plans, multiple-choice questions, and
permission prompts - in order, plays a distinct sound the instant a decision needs you, and
lets you answer and control the speech without looking. Run a full session with the screen
off.

- **Ordered narration** - prose, plans, questions, and permissions are spoken in order, never out of sequence.
- **Per-decision earcons** - a distinct sound the moment a question, permission, or error appears.
- **Selection by number** - answer prompts with the option's number; no key injection.
- **Global hotkeys** - replay the latest response, flush, mute, and cycle between sessions, hands-free (stop, repeat, skip, rate, and more are CLI commands).
- **Session manager** - a Sessions tab in the settings page: name each Claude Code session, mute it, or give it its own voice.
- **Speech volume** - a settings slider from 25 to 200 percent, so Sonara can sit quieter or louder than your other apps.
- **Lightweight** - the daemon runs on Python's standard library; the one-time `sonara install` fetches the Windows speech engine (PyWinRT) for you, and neural voices stay optional.

## Requirements

- Windows 10 or 11 (Sonara uses the built-in Windows speech engine and `winsound`
  for earcons).
- Python 3.9 or newer on your PATH - install it from
  [python.org](https://www.python.org/downloads/windows/) (the Microsoft Store stub is
  not used). Sonara picks the best `python` >= 3.9 automatically.
- Claude Code 2.1.162 or newer.

Python is the only thing you need beforehand: `sonara install` (below) fetches the Windows
speech engine (PyWinRT) for you with one `pip` step.

## Install

Sonara installs from a Claude Code marketplace, then one required setup command wires up
the speech engine, autostart, hooks, and hotkeys.

1. Add the marketplace: `/plugin marketplace add Maxaubert/sonara` (or, in a shell,
   `claude plugin marketplace add Maxaubert/sonara`).
2. Install the plugin: `/plugin install sonara@sonara` (or
   `claude plugin install sonara@sonara`). The marketplace is named `sonara`, so the
   install target is `sonara@sonara`.
3. **Run `/sonara:install`** - the required one-time setup. It installs the Windows speech
   engine (PyWinRT) into your Python, copies the runtime to `~/.sonara/app`, and registers
   autostart, the Claude Code hooks, and global hotkeys. Each step is printed; it can take
   a minute (it downloads the speech-engine packages).
4. Start a new Claude Code session and run `/sonara:doctor` to confirm everything is green.
   You'll hear Claude read its output from then on.

For local development you can skip the marketplace and load the repo per session with
`claude --plugin-dir <path-to-sonara>`.

If you already have `sonara` on your PATH, the CLI equivalent of step 3 is:

```bash
sonara install
```

`sonara install` resolves the best `python` >= 3.9, **installs the speech engine
(PyWinRT)**, **copies the runtime to `~/.sonara/app`** (so it survives plugin
auto-updates), registers the Windows autostart entry, and places the `sonara` launcher on
your PATH. If it can't install PyWinRT (for example, no network), it prints the exact
`pip` command to run and exits non-zero, so it never silently leaves you without speech.
After a plugin update, Sonara says once - *"Sonara was updated. Run /sonara:install
to apply."* - so you can refresh the copy.

### Development

Contributors can run the test suite from a venv:

```powershell
python -m venv .venv; .venv\Scripts\pip install -e '.[dev]'
.venv\Scripts\python -m pytest -q
```

The public install path above does **not** use `pip` - the venv is for tests only.

## Enhanced-voice setup (recommended)

Sonara defaults to the best natural/neural English voice it can find. Windows natural voices
sound much better and are free and offline. To install one:

1. Open **Settings → Time & language → Speech**.
2. Under **Manage voices**, click **Add voices**.
3. Pick an English voice marked **(Natural)** - e.g. *Microsoft Ava (Natural)* or
   *Microsoft Andrew (Natural)* - and download it.
4. Run `sonara doctor` to confirm Sonara picks it up, or set it explicitly:

```powershell
sonara voice "Microsoft Ava (Natural)"
```

## Controls and slash commands

Control is via global hotkeys (work even mid-speech), the `sonara` CLI, and namespaced slash
commands inside a session.

### Global hotkeys

Default modifier is **Win+Alt** (rebind on the settings page or in
`~/.sonara/keymap.json`). The daemon registers these as Windows global hotkeys, so no
extra accessibility permission is needed. A hotkey must include Ctrl, Alt or Win.

> **Changed in 0.7.0:** the defaults moved from Ctrl+Alt to Win+Alt, with new keys
> (Ctrl+Alt+Up/Down/M/P became Win+Alt+Home/End/S/N). An existing install keeps the
> bindings in its `~/.sonara/keymap.json`. To switch to the new defaults, press
> **Reset hotkeys to defaults** on the settings page's Hotkeys tab, or run
> `sonara keymap --reset`.

**Why Win+Alt:** Windows sends AltGr as Ctrl+Alt, so on layouts with AltGr characters
(German, Norwegian, Polish and others) a Ctrl+Alt hotkey takes that character away: with
Ctrl+Alt+M bound to mute, AltGr+M no longer types µ. A Win chord never matches AltGr.
If you keep or bind a clashing Ctrl+Alt hotkey, `sonara doctor` names each one and the
character it eats in a **[warn] AltGr** row, and the settings page shows the same warning
next to the binding.

**Keys Windows already owns:** Windows 11 uses Win+Alt+Up/Down (snap a window to the top
or bottom half), Win+Alt+M/R/G/T/PrtScn (Game Bar), Win+Alt+B (HDR), Win+Alt+D (date and
time), Win+Alt+K (microphone mute), Win+Alt+H (voice typing) and Win+Alt+digits (jump
lists). That is why restart and flush use Home and End instead of Up and Down, and mute
uses S instead of M. If another app already holds a chord, `sonara doctor` reports it in
its **hotkey chords** row; rebind that action on the settings page.

Only these actions are bound by default (kept minimal so Sonara doesn't hog
hotkeys). `pause`, `faster`, and `slower` are valid actions but ship **unbound** –
add a key in `~/.sonara/keymap.json` if you want one. Stop, repeat and skip
live in the CLI below.

Sonara always reads one message, the latest: there is no stepping back through
paragraphs or older turns. Win+Alt+Home simply starts the latest response over.

| Hotkey | Effect |
|---|---|
| Win+Alt+Home | Replay the latest response from the top (in summary mode, re-read the last summary) |
| Win+Alt+End | Flush – silence everything queued, in every session, and go quiet (Win+Alt+Home brings the latest response back) |
| Win+Alt+S | Cycle mute: Unmuted → Muted (speech) → Super muted (speech + beeps) |
| Win+Alt+N | Cycle to the next session in a fixed round-robin (resumes an unread session, replays a read one). Says "Session changed: &lt;folder&gt;." |

### Selecting options

When a question, permission prompt, or plan (`AskUserQuestion` / permission /
`ExitPlanMode`) appears, choose an option by pressing its **number (1-9)**, or `Esc` to
cancel - using Claude Code's native numeric selection, no key injection. For a
**multi-select** question, press each option's number (or `Space` on the highlighted item),
then `Enter` to confirm. If a question has **more than nine options**, numbers cover 1-9;
use the **arrow keys** plus `Enter` for the tenth and beyond. Sonara speaks these cues when
they apply.

### Slash commands and CLI

Day-to-day tuning (voice, rate, summaries, audio, sessions, hotkeys) lives in the
**settings page** (`/sonara:settings`). Only the lifecycle essentials ship as
`/sonara:` slash commands; everything else remains available as a CLI subcommand
(run `sonara <cmd>` in a terminal).

| Slash command | CLI | Effect |
|---|---|---|
| `/sonara:install` | `sonara install` | One-time setup: speech engine (PyWinRT), autostart, global hotkeys, control CLI (copies runtime to `~/.sonara/app`) |
| `/sonara:settings` | `sonara settings` | Open the browser settings page (voice, rate, summary, audio, sessions, hotkeys, daemon) |
| `/sonara:start` | `sonara start` | Start the daemon (clears a previous shutdown; the settings page cannot do this because the daemon serves it) |
| `/sonara:doctor` | `sonara doctor` | Run all health checks |
| `/sonara:uninstall` | `sonara uninstall` | Remove the autostart entry, launcher, and `~/.sonara/app` (keeps your settings) |

CLI-only (no slash command): `sonara status`, `verbosity`, `voice`, `voices`
(install/remove Kokoro), `rate`, `minqueue`, `summary`,
`audio-mode`, `duck-level`, `keymap` (`--reset` restores the default hotkeys), `repeat`, `skip`, `stop`, `shutdown`, `cleanup`.

## Verbosity

Three live-switchable levels (earcons fire in **all** of them):

- **everything** (default) - prose narration, questions, plans, permissions, *and* brief
  tool announcements (a short summary of what's running, e.g. "Running git status").
- **medium** - prose narration plus decisions (questions / plans / permissions); **drops**
  routine tool announcements.
- **quiet** - decisions only (questions / plans / permissions); drops both tool
  announcements **and** prose narration. Earcons still fire at every level.

## Summary mode

`sonara summary on` switches Sonara to a recap style: instead of narrating a whole
response, Sonara waits for the message to finish and reads a 1-2 sentence summary.
Decisions (questions, plans, permission prompts) are still read in full, every
earcon still fires, and Win+Alt+Home re-reads the last summary.

How it works: when a turn finishes, Sonara runs a separate, throwaway
`claude -p` call (default model: Haiku, tool-disabled) with only that message's
text and speaks the result. Your main Claude session is untouched, and nothing is
added to its context. The recap call reuses your existing Claude Code login and its
tokens count against your plan (one small call per finished message); expect a few
seconds between the message finishing and the recap being spoken. If the call
fails (offline, timeout), Sonara plays a brief cue and stays quiet. Summary mode
is off by default.

## How ordering works

Sonara's voice never jumps ahead of you. Spoken content is **strictly first-in, first-out**: a
question, plan, or permission is voiced *in its natural place* - after the prose that
explains it - so if the voice is mid-sentence when a permission appears, you still hear the
remaining sentences first, then the permission. What *is* instant is the **alert**: the
moment any decision appears, a short distinct earcon plays immediately (a different sound for
permission, choice, plan, error, turn-done, and ready), while the spoken detail waits its
turn in the queue. Claude Code blocks on the prompt until you respond, so hearing the
context first costs nothing. "Higher priority" therefore means *"alert you instantly with a
sound,"* never *"speak it out of order."*

## Per-session behavior

Sonara tracks a single **foreground** session (set by `SessionStart` and each
`UserPromptSubmit`). Only the foreground session is *spoken*; if you run multiple sessions,
background sessions still fire decision **earcons** so you are alerted, but their prose and
decision text are not read aloud until you bring that session forward. Submitting a new
prompt or stopping flushes the queue, so the voice always resumes at what is current.

To manually cycle the voice to another session without switching windows, press
**Win+Alt+N**. Sonara advances to the next session in a
fixed round-robin order, plays a short chime, and says "Session changed: &lt;folder&gt;." An
unread session resumes from where it left off; a fully-read session is replayed from the
top.

## Doctor and troubleshooting

Run `sonara doctor` first - it reports each check as pass/fail. Common issues:

- **No speech at all.** Confirm `sonara status` shows your session as the foreground. The
  daemon starts lazily on the first hook; if the socket is unreachable, run `sonara install`
  to (re)load the daemon (`sonara doctor` tells you whether the socket is reachable), or
  check `~/.sonara/speechd.log`.
- **Robotic voice.** No enhanced voice is installed; see *Enhanced-voice setup* above.
- **Hooks not firing.** Re-enable `sonara` via `/plugin` (or re-launch with
  `claude --plugin-dir /path/to/sonara`), then run `sonara doctor` and confirm the
  `plugin hooks.json` check passes.
- **Speech too fast/slow.** `sonara rate 180` (default is 200 wpm).
- **Too chatty.** `sonara verbosity medium` or `sonara verbosity quiet`.
- **Everything is stuck.** `sonara stop` clears the queue and cancels the current utterance.
- **Disk space after upgrading.** Sonara 0.6.1 removed the optional Chatterbox engine. If you
  had installed it, `sonara doctor` lists the leftover venv and model cache (several GB) and
  `sonara cleanup` deletes them. Your own voice clips in `~/.sonara/voices/` are kept. A saved
  Chatterbox voice now speaks as Kokoro's Heart (`af_heart`).

State, config, the daemon lockfile, and logs all live under `~/.sonara/`
(`config.json`, `daemon.lock`, `speechd.log`).

## Uninstall

To remove Sonara, disable the `sonara` plugin via `/plugin` (or stop passing
`--plugin-dir`), then run:

```powershell
sonara uninstall
```

`sonara uninstall` removes the Windows autostart entry and the `sonara` launcher.
It preserves your `~/.sonara/config.json` and
`~/.sonara/keymap.json` so your settings survive a reinstall.

The in-session equivalent is `/sonara:uninstall`. Uninstall also removes the
stable app copy at `~/.sonara/app`, and **preserves** your `config.json` and
`keymap.json`.

## Privacy

Sonara runs entirely on your own computer. It collects nothing, sends nothing over the network
(except, if you opt in, summary mode's local `claude -p` call described above),
and has no servers, telemetry, or analytics - the text it speaks is processed locally and is
never stored or transmitted. See [PRIVACY.md](PRIVACY.md) for the full details.

## License

MIT - see [LICENSE](LICENSE).
