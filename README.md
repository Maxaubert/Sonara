# Sonara

> The Windows line of Sonara, forked from [nimkimi/sonari](https://github.com/nimkimi/sonari) (the macOS line). Developed and released independently.

**Eyes-free text-to-speech for [Claude Code](https://claude.ai/code) on macOS — an
accessibility tool for blind and low-vision developers.**

Sonara reads Claude Code's output aloud — prose, plans, multiple-choice questions, and
permission prompts — in order, plays a distinct sound the instant a decision needs you, and
lets you answer and control the speech without looking. Run a full session with the screen
off.

- **Ordered narration** — prose, plans, questions, and permissions are spoken in order, never out of sequence.
- **Per-decision earcons** — a distinct sound the moment a question, plan, permission, or error appears.
- **Selection by number** — answer prompts with the option's number; no key injection.
- **Global hotkeys** — stop, repeat, skip, jump-to-decision, catch-up, rate, verbosity, re-read (work mid-speech).
- **Self-contained** — runs on the macOS system Python; no `pip`, no third-party packages.

## Requirements

- macOS (Sonara uses the built-in `say` and `afplay` commands).
- Python 3.9 or newer — macOS ships `/usr/bin/python3`, which is enough. Sonara
  picks the best `python3 >= 3.9` it can find automatically.
- Xcode Command Line Tools for global hotkeys — `xcode-select --install`. (Speech
  works without them; only the hotkeys need `swiftc`.)
- Claude Code 2.1.162 or newer.

## Install

Sonara installs from a Claude Code marketplace. You start hearing Claude as soon as the
plugin is enabled; one more command turns on global hotkeys and autostart.

1. Add the marketplace: `/plugin marketplace add Maxaubert/sonara` (or, in a shell,
   `claude plugin marketplace add Maxaubert/sonara`).
2. Install the plugin: `/plugin install sonara@sonara` (or
   `claude plugin install sonara@sonara`). The marketplace is named `sonara`, so the
   install target is `sonara@sonara`. You will start hearing Claude immediately — the
   daemon lazy-starts on the first hook.
3. Run `/sonara:install` from inside Claude Code to finish setup (each step is printed and
   spoken). Until you run it, every new session Sonara reminds you once: *"Sonara is reading
   aloud. To enable hotkeys and autostart, run /sonara:install."*
4. Run `/sonara:doctor` to confirm everything is green (the only expected failure is
   `swiftc` / Xcode Command Line Tools on a machine without them — speech still works;
   only the hotkeys need them).

For local development you can skip the marketplace and load the repo per session with
`claude --plugin-dir <path-to-sonara>`.

If you already have `sonara` on your PATH, the CLI equivalent of step 3 is:

```bash
sonara install
```

`sonara install` resolves the best `python3 >= 3.9`, **copies the runtime to
`~/.sonara/app`** (so it survives plugin auto-updates), builds the hotkey
daemon, writes both LaunchAgents, and places the `~/.local/bin/sonara` launcher.
After a plugin update, Sonara says once — *"Sonara was updated. Run /sonara:install
to apply."* — so you can refresh the copy.

### Development

Contributors can run the test suite from a venv:

```bash
python3 -m venv .venv && .venv/bin/pip install -e '.[dev]'
.venv/bin/python -m pytest -q
```

The public install path above does **not** use `pip` — the venv is for tests only.

## Enhanced-voice setup (recommended)

Sonara defaults to the best enhanced/neural English voice it can find and falls back to
**Samantha**. Enhanced voices sound dramatically better and are free and offline. To install
one:

1. Open **System Settings → Accessibility → Spoken Content**.
2. Click **System Voice → Manage Voices…**.
3. Pick an English voice marked **(Enhanced)** or **(Premium)** — e.g. *Ava (Premium)*,
   *Zoe (Premium)*, or *Allison* — and download it.
4. Run `sonara doctor` to confirm Sonara picks it up, or set it explicitly:

```bash
sonara voice "Ava (Premium)"
```

## Controls and slash commands

Control is via global hotkeys (work even mid-speech), the `sonara` CLI, and namespaced slash
commands inside a session.

### Global hotkeys

Default modifier is **Ctrl+Cmd** (rebindable via `~/.sonara/keymap.json`). A tiny Swift
helper registers these with Carbon `RegisterEventHotKey`, so no macOS accessibility
permission is needed.

| Hotkey | Effect |
|---|---|
| Ctrl+Cmd+S | Stop now and clear the queue |
| Ctrl+Cmd+R | Re-speak the entire last message |
| Ctrl+Cmd+. | Skip the current item |
| Ctrl+Cmd+D | Jump to the pending decision |
| Ctrl+Cmd+L | Replay everything you haven't heard (after stop, or from a session you left), then mark it heard |
| Ctrl+Cmd+] | Speak faster |
| Ctrl+Cmd+[ | Speak slower |
| Ctrl+Cmd+V | Cycle verbosity (everything / medium / quiet) |
| Ctrl+Cmd+O | Re-read the current prompt's options (numbers, descriptions, multi-select announce) |
| Ctrl+Cmd+P | Cycle the voice to the next session in a fixed round-robin (resumes an unread session, replays a read one). Says "Session changed: &lt;folder&gt;." |

### Selecting options

When a question, permission prompt, or plan (`AskUserQuestion` / permission /
`ExitPlanMode`) appears, choose an option by pressing its **number (1-9)**, or `Esc` to
cancel — using Claude Code's native numeric selection, no key injection. For a
**multi-select** question, press each option's number (or `Space` on the highlighted item),
then `Enter` to confirm. If a question has **more than nine options**, numbers cover 1-9;
use the **arrow keys** plus `Enter` for the tenth and beyond. Sonara speaks these cues when
they apply.

### Slash commands and CLI

| Slash command | CLI | Effect |
|---|---|---|
| `/sonara:install` | `sonara install` | One-time setup: autostart, global hotkeys, control CLI (copies runtime to `~/.sonara/app`) |
| `/sonara:uninstall` | `sonara uninstall` | Remove LaunchAgents, hotkey helper, launcher, and `~/.sonara/app` (keeps your settings) |
| `/sonara:status` | `sonara status` | Show voice, rate, verbosity, min-queue, foreground session, queue length |
| `/sonara:verbosity <level>` | `sonara verbosity <level>` | Set `everything` / `medium` / `quiet` |
| `/sonara:voice <name>` | `sonara voice <name>` | Set the `say` voice |
| `/sonara:rate <wpm>` | `sonara rate <wpm>` | Set words-per-minute |
| `/sonara:minqueue <n>` | `sonara minqueue <n>` | Batch this many items before reading (1-10; 1 = read immediately) |
| `/sonara:repeat` | `sonara repeat` | Re-speak the last item |
| `/sonara:skip` | `sonara skip` | Skip the current item |
| `/sonara:stop` | `sonara stop` | Stop now and clear the queue |
| `/sonara:doctor` | `sonara doctor` | Run all health checks |
| `/sonara:keymap` | `sonara keymap` | Show the active global hotkey bindings |

## Verbosity

Three live-switchable levels (earcons fire in **all** of them):

- **everything** (default) — prose narration, questions, plans, permissions, *and* brief
  tool announcements (a short summary of what's running, e.g. "Running git status").
- **medium** — prose narration plus decisions (questions / plans / permissions); **drops**
  routine tool announcements.
- **quiet** — decisions only (questions / plans / permissions); drops both tool
  announcements **and** prose narration. Earcons still fire at every level.

## How ordering works

Sonara's voice never jumps ahead of you. Spoken content is **strictly first-in, first-out**: a
question, plan, or permission is voiced *in its natural place* — after the prose that
explains it — so if the voice is mid-sentence when a permission appears, you still hear the
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
**Ctrl+Cmd+P** (macOS) or the platform equivalent. Sonara advances to the next session in a
fixed round-robin order, plays a short chime, and says "Session changed: &lt;folder&gt;." An
unread session resumes from where it left off; a fully-read session is replayed from the
top.

## Doctor and troubleshooting

Run `sonara doctor` first — it reports each check as pass/fail. Common issues:

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

State, config, the socket, and logs all live under `~/.sonara/`
(`config.json`, `speechd.sock`, `speechd.log`).

## Uninstall

To remove Sonara, disable the `sonara` plugin via `/plugin` (or stop passing
`--plugin-dir`), then run:

```bash
sonara uninstall
```

`sonara uninstall` removes the LaunchAgents, the hotkey helper, and the
`~/.local/bin/sonara` launcher. It preserves your `~/.sonara/config.json` and
`~/.sonara/keymap.json` so your settings survive a reinstall.

The in-session equivalent is `/sonara:uninstall`. Uninstall also removes the
stable app copy at `~/.sonara/app`, and **preserves** your `config.json` and
`keymap.json`.

## Privacy

Sonara runs entirely on your own Mac. It collects nothing, sends nothing over the network,
and has no servers, telemetry, or analytics — the text it speaks is processed locally and is
never stored or transmitted. See [PRIVACY.md](PRIVACY.md) for the full details.

## License

MIT — see [LICENSE](LICENSE).
