# Sonari

**Eyes-free text-to-speech for [Claude Code](https://claude.ai/code) on macOS.**

Sonari speaks everything Claude Code does — prose, plans, multiple-choice questions, and
permission prompts — so you can run a full session **with the screen off**. It is a
ground-up rebuild of the old `claude-tts` tool: one speech daemon, one ordered queue, one
`say` voice at a time, and a distinct sound (an *earcon*) the instant any decision appears.

> **Phase 1 (this release) is the output pipeline:** you *hear* everything, in order,
> reliably. Answering questions and approving actions still uses Claude Code's own keyboard
> picker. Fully eyes-free *selection* via global hotkeys is Phase 2.

## The goal

A blind or low-vision developer should be able to use Claude Code without looking at the
screen. Sonari's job in Phase 1 is to make sure nothing important is ever silent or
out-of-order: you always hear the prose that explains a decision *before* the decision, and
a short sound alerts you the moment a question, plan, or permission is waiting.

## Requirements

- macOS (Sonari uses the built-in `say` and `afplay` commands).
- Python 3.10 or newer (`python3 --version`).
- Claude Code 2.1.162 or newer.
- No third-party Python packages at runtime. `pytest` is only needed to run the tests.

## Install

```bash
git clone https://github.com/nimkimi/claude-tts ~/projects/claude-tts
cd ~/projects/claude-tts
pip install -e .
```

`pip install -e .` installs the `sonari` package and creates the `sonari` command on your
PATH.

There is no `claude plugin add` command. To load Sonari as a Claude Code plugin (it
registers its hooks declaratively — no hand-editing of `settings.json`), use one of:

```bash
# Per session: point Claude Code at this repo's plugin directory.
claude --plugin-dir ~/projects/claude-tts
```

Or register this repo as a **local plugin marketplace** and enable `sonari` from the
`/plugin` menu so it loads automatically in every session.

Verify everything is wired up:

```bash
sonari doctor
```

`doctor` checks: an enhanced voice is installed, the `say`/`afplay` binaries exist, the
speech daemon can start and its socket is reachable, the plugin manifests are valid, and the
seven Phase 1 hooks are registered. Start a Claude Code session and you should hear a
**ready** earcon.

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

In Phase 1, control is via the `sonari` CLI and namespaced slash commands inside a session.
(Global hotkeys that work even mid-speech arrive in Phase 2.)

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
- **Hooks not firing.** Re-launch with `claude --plugin-dir ~/projects/claude-tts` (or
  re-enable `sonari` via `/plugin`) and verify with `sonari doctor` that all seven hooks are
  registered.
- **Speech too fast/slow.** `sonari rate 180` (default is 200 wpm).
- **Too chatty.** `sonari verbosity medium` or `sonari verbosity quiet`.
- **Everything is stuck.** `sonari stop` clears the queue and cancels the current utterance.

State, config, the socket, and logs all live under `~/.sonari/`
(`config.json`, `speechd.sock`, `speechd.log`).

## Uninstall and migration from legacy

To remove Sonari, disable the `sonari` plugin via `/plugin` (or stop passing
`--plugin-dir`), then run:

```bash
sonari uninstall
```

`sonari uninstall` also cleans up a **prior legacy `claude-tts` install** if one is present on
your machine: it removes the `alias claude=claude-speak` line and the `~/.local/bin` PATH
export from your `~/.zshrc`, removes the three legacy hooks
(`claude-tts-permission.sh`, `claude-tts-pre-tool.sh`, `claude-tts-stop.sh`) from
`~/.claude/settings.json`, and deletes `~/.local/bin/claude-speak`,
`~/.local/bin/claude-tts`, `~/.claude-tts-enabled`, and `~/.claude-tts-pos`. The new Sonari
design uses **no shell alias and no `~/.zshrc` edits at all**. (The legacy code is preserved
at git tag `v0-legacy-pty` if you ever need it.)

## What's next (Phase 2)

Global keyboard hotkeys for live speech control (skip, jump-to-decision, catch-up) and 100%
eyes-free **selection** — pick any question option and approve plans and permissions without
ever looking at the screen.
