# Sonari

**Eyes-free text-to-speech for [Claude Code](https://claude.ai/code) on macOS.**

Sonari speaks everything Claude Code does — prose, plans, multiple-choice questions, and
permission prompts — **in order**, and plays a distinct sound (an *earcon*) the instant any
decision needs you, so you can run a full session **with the screen off**. You *answer*
without looking by pressing the option's number (1-9), or `Esc` to cancel; and global
speech-control hotkeys (stop, repeat, skip, jump-to-decision, catch-up, rate, verbosity,
re-read) work even mid-speech.

## Why Sonari

Claude Code lives in a fast-changing terminal interface — exactly the kind of thing that's
awkward to follow by ear. Screen readers *can* do it, but they're demanding to learn and
configure, and they tend to fight a live, redrawing TUI: too chatty, often out of order, and
easy to lose your place in.

Plenty of **low-vision** developers don't use a screen reader at all — they get by with a
magnifier and a lot of squinting — which makes a busy terminal *extra* tiring.

Sonari is for them, and for anyone who'd rather not stare at the screen. A blind or low-vision
developer should be able to use Claude Code without looking. Instead of narrating the whole
interface, Sonari speaks just what matters, in the right order — you always hear the prose
that explains a decision *before* the decision — alerts you with a quick sound the moment a
question, plan, or permission is waiting, and lets you choose any option by typing its number.
The point is to work with Claude Code **relaxed, by ear** — no steep screen-reader setup, no
eye strain.

## Requirements

- macOS (Sonari uses the built-in `say` and `afplay` commands).
- Python 3.9 or newer — macOS ships `/usr/bin/python3`, which is enough. Sonari
  picks the best `python3 >= 3.9` it can find automatically.
- Xcode Command Line Tools for global hotkeys — `xcode-select --install`. (Speech
  works without them; only the hotkeys need `swiftc`.)
- Claude Code 2.1.162 or newer.

## Install

Sonari installs from a Claude Code marketplace. You start *hearing* Claude as soon
as the plugin is enabled; one more command turns on global hotkeys and autostart —
and you can do all of it eyes-free from inside Claude Code.

1. Add the marketplace: `/plugin marketplace add nimkimi/sonari` (or, in a shell,
   `claude plugin marketplace add nimkimi/sonari`).
2. Install the plugin: `/plugin install sonari@sonari` (or
   `claude plugin install sonari@sonari`). The marketplace is named `sonari`, so the
   install target is `sonari@sonari`. You will start hearing Claude immediately — the
   daemon lazy-starts on the first hook.
3. Run `/sonari:install` from inside Claude Code to finish setup. Each step is printed
   (and spoken) so you can follow along eyes-free. Until you run it, every new session
   Sonari speaks a one-time reminder: *"Sonari is reading aloud. To enable hotkeys and
   autostart, run /sonari:install."*
4. Run `/sonari:doctor` to confirm everything is green (the only expected failure is
   `swiftc` / Xcode Command Line Tools on a machine without them — speech still works;
   only the hotkeys need them).

For local development you can skip the marketplace and load the repo per session with
`claude --plugin-dir <path-to-sonari>`.

If you already have `sonari` on your PATH, the CLI equivalent of step 3 is:

```bash
sonari install
```

`sonari install` resolves the best `python3 >= 3.9`, **copies the runtime to
`~/.sonari/app`** (so it survives plugin auto-updates), builds the hotkey
daemon, writes both LaunchAgents, and places the `~/.local/bin/sonari` launcher.
After a plugin update, Sonari says once — *"Sonari was updated. Run /sonari:install
to apply."* — so you can refresh the copy.

### Development

Contributors can run the test suite from a venv:

```bash
python3 -m venv .venv && .venv/bin/pip install -e '.[dev]'
.venv/bin/python -m pytest -q
```

The public install path above does **not** use `pip` — the venv is for tests only.

## Enhanced-voice setup (recommended)

Sonari defaults to the best enhanced/neural English voice it can find and falls back to
**Samantha**. Enhanced voices sound dramatically better and are free and offline. To install
one:

1. Open **System Settings → Accessibility → Spoken Content**.
2. Click **System Voice → Manage Voices…**.
3. Pick an English voice marked **(Enhanced)** or **(Premium)** — e.g. *Ava (Premium)*,
   *Zoe (Premium)*, or *Allison* — and download it.
4. Run `sonari doctor` to confirm Sonari picks it up, or pin it explicitly:

```bash
sonari voice "Ava (Premium)"
```

## Controls and slash commands

Control is via global hotkeys (work even mid-speech), the `sonari` CLI, and namespaced slash
commands inside a session.

### Global hotkeys

Default modifier is **Ctrl+Cmd** (rebindable via `~/.sonari/keymap.json`). A tiny Swift
helper registers these with Carbon `RegisterEventHotKey`, so no macOS accessibility
permission is needed.

| Hotkey | Effect |
|---|---|
| Ctrl+Cmd+S | Stop now and clear the queue |
| Ctrl+Cmd+R | Repeat the last item |
| Ctrl+Cmd+. | Skip the current item |
| Ctrl+Cmd+D | Jump to the pending decision |
| Ctrl+Cmd+L | Catch up to live (drop the backlog, jump to now) |
| Ctrl+Cmd+] | Speak faster |
| Ctrl+Cmd+[ | Speak slower |
| Ctrl+Cmd+V | Cycle verbosity (everything / medium / quiet) |
| Ctrl+Cmd+O | Re-read the current options |

### Eyes-free selection

When a question, permission prompt, or plan (`AskUserQuestion` / permission /
`ExitPlanMode`) appears, choose an option by pressing its **number (1-9)**, or `Esc` to
cancel — using Claude Code's native numeric selection, no key injection. For a
**multi-select** question, press each option's number (or `Space` on the highlighted item),
then `Enter` to confirm. If a question has **more than nine options**, numbers cover 1-9;
use the **arrow keys** plus `Enter` for the tenth and beyond. Sonari speaks these cues when
they apply.

### Slash commands and CLI

| Slash command | CLI | Effect |
|---|---|---|
| `/sonari:install` | `sonari install` | One-time setup: autostart, global hotkeys, control CLI (copies runtime to `~/.sonari/app`) |
| `/sonari:uninstall` | `sonari uninstall` | Remove LaunchAgents, hotkey helper, launcher, and `~/.sonari/app` (keeps your settings) |
| `/sonari:status` | `sonari status` | Show voice, rate, verbosity, foreground session, queue length |
| `/sonari:verbosity <level>` | `sonari verbosity <level>` | Set `everything` / `medium` / `quiet` |
| `/sonari:voice <name>` | `sonari voice <name>` | Set the `say` voice |
| `/sonari:rate <wpm>` | `sonari rate <wpm>` | Set words-per-minute |
| `/sonari:repeat` | `sonari repeat` | Re-speak the last item |
| `/sonari:skip` | `sonari skip` | Skip the current item |
| `/sonari:stop` | `sonari stop` | Stop now and clear the queue |
| `/sonari:doctor` | `sonari doctor` | Run all health checks |
| `/sonari:keymap` | `sonari keymap` | Show the active global hotkey bindings |

## Verbosity

Three live-switchable levels (earcons fire in **all** of them):

- **everything** (default) — prose narration, questions, plans, permissions, *and* brief
  tool announcements (a short summary of what's running, e.g. "Running git status").
- **medium** — prose narration plus decisions (questions / plans / permissions); **drops**
  routine tool announcements.
- **quiet** — decisions only (questions / plans / permissions); drops both tool
  announcements **and** prose narration. Earcons still fire at every level.

## How ordering works

Sonari's voice never jumps ahead of you. Spoken content is **strictly first-in, first-out**: a
question, plan, or permission is voiced *in its natural place* — after the prose that
explains it — so if the voice is mid-sentence when a permission appears, you still hear the
remaining sentences first, then the permission. What *is* instant is the **alert**: the
moment any decision appears, a short distinct earcon plays immediately (a different sound for
permission, choice, plan, error, turn-done, and ready), while the spoken detail waits its
turn in the queue. Claude Code blocks on the prompt until you respond, so hearing the
context first costs nothing. "Higher priority" therefore means *"alert you instantly with a
sound,"* never *"speak it out of order."*

## Per-session behavior

Sonari tracks a single **foreground** session (set by `SessionStart` and each
`UserPromptSubmit`). Only the foreground session is *spoken*; if you run multiple sessions,
background sessions still fire decision **earcons** so you are alerted, but their prose and
decision text are not read aloud until you bring that session forward. Submitting a new
prompt or stopping flushes the queue, so the voice always resumes at what is current.

## Doctor and troubleshooting

Run `sonari doctor` first — it reports each check as pass/fail. Common issues:

- **No speech at all.** Confirm `sonari status` shows your session as the foreground. The
  daemon starts lazily on the first hook; if the socket is unreachable, run `sonari install`
  to (re)load the daemon (`sonari doctor` tells you whether the socket is reachable), or
  check `~/.sonari/speechd.log`.
- **Robotic voice.** No enhanced voice is installed; see *Enhanced-voice setup* above.
- **Hooks not firing.** Re-enable `sonari` via `/plugin` (or re-launch with
  `claude --plugin-dir /path/to/sonari`), then run `sonari doctor` and confirm the
  `plugin hooks.json` check passes.
- **Speech too fast/slow.** `sonari rate 180` (default is 200 wpm).
- **Too chatty.** `sonari verbosity medium` or `sonari verbosity quiet`.
- **Everything is stuck.** `sonari stop` clears the queue and cancels the current utterance.

State, config, the socket, and logs all live under `~/.sonari/`
(`config.json`, `speechd.sock`, `speechd.log`).

## Uninstall

To remove Sonari, disable the `sonari` plugin via `/plugin` (or stop passing
`--plugin-dir`), then run:

```bash
sonari uninstall
```

`sonari uninstall` removes the LaunchAgents, the hotkey helper, and the
`~/.local/bin/sonari` launcher. It preserves your `~/.sonari/config.json` and
`~/.sonari/keymap.json` so your settings survive a reinstall.

The in-session equivalent is `/sonari:uninstall`. Uninstall also removes the
stable app copy at `~/.sonari/app`, and **preserves** your `config.json` and
`keymap.json`.

## Privacy

Sonari runs entirely on your own Mac. It collects nothing, sends nothing over the network,
and has no servers, telemetry, or analytics — the text it speaks is processed locally and is
never stored or transmitted. See [PRIVACY.md](PRIVACY.md) for the full details.
