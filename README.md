# Sonari

**Eyes-free text-to-speech for [Claude Code](https://claude.ai/code) on macOS.**

Sonari speaks everything Claude Code does — prose, plans, multiple-choice questions, and
permission prompts — so you can run a full session **with the screen off**. Under the hood
it's deliberately simple: one speech daemon, one ordered queue, one `say` voice at a time,
and a distinct sound (an *earcon*) the instant any decision appears.

> **Fully eyes-free today:** you *hear* everything in order — prose, plans, multiple-choice
> questions, and permission prompts, with a distinct earcon the instant any decision appears
> — and you *answer* without looking by pressing the option's number (1-9), `Esc` to cancel.
> Global speech-control hotkeys (stop, repeat, skip, jump-to-decision, catch-up, rate,
> verbosity, re-read options) work even mid-speech.

## Why Sonari

Claude Code lives in a fast-changing terminal interface — exactly the kind of thing that's
awkward to follow by ear. Screen readers *can* do it, but they're demanding to learn and
configure, and they tend to fight a live, redrawing TUI: too chatty, often out of order, and
easy to lose your place in.

Plenty of **low-vision** developers don't use a screen reader at all — they get by with a
magnifier and a lot of squinting — which makes a busy terminal *extra* tiring. Sonari is for
them, and for anyone who'd simply rather not stare at the screen. Instead of narrating the
whole interface, it speaks just what matters, in the right order, and plays a quick sound the
moment a decision needs you. The point is to work with Claude Code **relaxed, by ear** — no
steep screen-reader setup, no eye strain.

## The goal

A blind or low-vision developer should be able to use Claude Code without looking at the
screen. Sonari makes sure nothing important is ever silent or out-of-order: you always hear
the prose that explains a decision *before* the decision, a short sound alerts you the moment
a question, plan, or permission is waiting, and you choose any option by typing its number —
no screen needed.

## Requirements

- macOS (Sonari uses the built-in `say` and `afplay` commands).
- Python 3.9 or newer — macOS ships `/usr/bin/python3`, which is enough. Sonari
  picks the best `python3 >= 3.9` it can find automatically.
- Xcode Command Line Tools for global hotkeys — `xcode-select --install`. (Speech
  works without them; only the hotkeys need `swiftc`.)
- Claude Code 2.1.162 or newer.
- No third-party Python packages at runtime, and no `pip` install. `pytest` is
  only needed to run the tests.

## Install

Sonari is a self-contained Claude Code plugin: it ships its own source and runs
on the macOS system Python with no `pip` install.

1. Add the marketplace and install the plugin:

```bash
claude plugin marketplace add nimkimi/sonari
claude plugin install sonari@sonari
```

   (For local development, you can instead run a session with
   `claude --plugin-dir /path/to/sonari`.)
2. Run the one-time installer (this needs Xcode Command Line Tools for the
   hotkeys — `xcode-select --install`):

```bash
sonari install
```

`sonari install` resolves the best `python3 >= 3.9`, builds the hotkey daemon
locally with `swiftc` (no notarization needed), writes both LaunchAgents with
absolute paths, and places a `~/.local/bin/sonari` launcher so the `sonari`
command works in every shell. If `~/.local/bin` is not on your PATH, the
installer prints the exact line to add.

Verify everything is wired up:

```bash
sonari doctor
```

`doctor` reports each check pass/fail: an enhanced voice, `say`/`afplay`,
`python3 >= 3.9`, the resolved plugin path, the speech and hotkey LaunchAgents,
the daemon socket, the `~/.local/bin/sonari` launcher, and the plugin hooks.
Start a Claude Code session and you should hear a **ready** earcon.

### Development

Contributors can run the test suite from a venv:

```bash
python3 -m venv .venv && .venv/bin/pip install -e .[dev]
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
| Ctrl+Cmd+L | Catch up (speak what you missed) |
| Ctrl+Cmd+] | Speak faster |
| Ctrl+Cmd+[ | Speak slower |
| Ctrl+Cmd+V | Cycle verbosity (everything / medium / quiet) |
| Ctrl+Cmd+O | Re-read the current options |

### Eyes-free selection

When a question, permission prompt, or plan (`AskUserQuestion` / permission /
`ExitPlanMode`) appears, choose an option by pressing its **number (1-9)**, or `Esc` to
cancel — using Claude Code's native numeric selection, no key injection.

### Slash commands and CLI

| Slash command | CLI | Effect |
|---|---|---|
| `/sonari:status` | `sonari status` | Show voice, rate, verbosity, foreground session, queue length |
| `/sonari:verbosity <level>` | `sonari verbosity <level>` | Set `everything` / `medium` / `quiet` |
| `/sonari:voice <name>` | `sonari voice <name>` | Set the `say` voice |
| `/sonari:rate <wpm>` | `sonari rate <wpm>` | Set words-per-minute |
| `/sonari:repeat` | `sonari repeat` | Re-speak the last item |
| `/sonari:stop` | `sonari stop` | Stop now and clear the queue |
| `/sonari:doctor` | `sonari doctor` | Run all health checks |

## Verbosity

Three live-switchable levels (earcons fire in **all** of them):

- **everything** (default) — prose narration, questions, plans, permissions, *and* brief
  tool announcements ("Running git status").
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
  daemon starts lazily on the first hook; if the socket is unreachable, run `sonari doctor` to
  restart it, or check `~/.sonari/speechd.log`.
- **Robotic voice.** No enhanced voice is installed; see *Enhanced-voice setup* above.
- **Hooks not firing.** Re-launch with `claude --plugin-dir /path/to/sonari` (or
  re-enable `sonari` via `/plugin`) and verify with `sonari doctor` that all seven hooks are
  registered.
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

## Privacy

Sonari runs entirely on your own Mac. It collects nothing, sends nothing over the network,
and has no servers, telemetry, or analytics — the text it speaks is processed locally and is
never stored or transmitted. See [PRIVACY.md](PRIVACY.md) for the full details.
