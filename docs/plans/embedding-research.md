# Embedding Sonara in PrismTerminal: integration research

_Dated 2026-10-01. Source: a read-only investigation of both repositories (Sonara at 8d90005 and
PrismTerminal 0.25.1); nothing was edited, committed or restarted. Line counts and file names
describe Sonara **before** Phase 0, so `daemon.py` and the Chatterbox modules no longer exist.
See "Phase 0 status" below for what has changed since._

My recommendation is to build audio mode natively inside PrismTerminal, in TypeScript, and to tidy Sonara first. A quick prototype can talk to the existing Sonara daemon, but only on your machine. Two parts of the plan are unverified: the speech engine choice (needs a quick test of latency and installer size) and whether the transcript files are written promptly enough to speak from.

## 1. PrismTerminal today
- **Stack:** Electron 43, React 19, TypeScript, Vite, Tailwind v4, xterm 6 and node-pty. It ships as a per-user NSIS installer through GitHub Releases, version 0.25.1.
- **Shared `core/` folder:** this is the terminal itself, and the Prism media viewer pulls it in as a pinned git tag (`github:Maxaubert/PrismTerminal#core-v0.16.0`). Anything placed in `core/` reaches both apps. Its contract (`core/README.md`): a feature lives in `core/` once, and each difference between the two apps is a declared field of `TermHostConfig`, which is your decision to add.
- **Sessions:** each tab is a plain shell (pwsh, cmd and so on) in a pty. Claude, Codex, aider and gemini are not hosted directly; they are found by checking which processes run under each tab's shell (`core/main/agentDetect.ts`, `agentPoll.ts`).
  - Claude's working state is read from the window title it sets (`TerminalPanel.tsx` around line 775).
  - Questions are spotted by matching text on screen (`core/renderer/lib/agentQuestion.ts`), which breaks if Claude rewords them.
  - `agentResume.ts` already reads Claude's saved session files on disk.
  - `ptyEnv` removes Claude's session environment variables before starting a shell.
- **Dictation is the template to copy.** It was built once in `core/` and a host turns it on with four lines.
  - Its engine (whisper.cpp) is an official program the app runs separately, talking to it on 127.0.0.1.
  - Every download is checked against a pinned SHA-256 and stored in `%LOCALAPPDATA%\PrismDictation`.
  - It is off by default, and off means no process and no download. It warms up before first use.
  - It has its own on-screen pill (`DictationPill`), a Settings page and a real end-to-end test.
  - `mediaPause.ts` already pauses whatever is playing in Windows, which matches Sonara's own pause feature.
- **Rules from its CLAUDE.md that limit the design:**
  - "This is a product for other people": a feature must bundle or download what it needs and work on a fresh Windows install.
  - node-pty is meant to be the only native Node module in either app.

## 2. Sonara: what's tied to Claude Code and what's reusable

| Part | Lines | Tied to Claude Code? | Reusable? |
|---|---|---|---|
| `hooks/hooks.json`, `bin/sonara-hook*`, `hooks_entry.py` | about 370 | Yes. `hooks_entry.py` is a pure translation from hook events (MessageDisplay text pieces, PreToolUse, Notification, Stop, SessionStart and so on) to Sonara messages | Only the message format |
| `cleaner.py`, `assembler.py` | 375 | No. Pure functions: strip markdown, split into sentences, summarise code blocks | Yes, easy to port to TypeScript, with tests |
| `queue.py`, `channel.py`, `router.py`, `sessions.py`, `history.py`, `session_prefs.py` | about 900 | Mostly no. Decides which session reads and keeps per-session queues | Yes. Mostly pure; `sessions.py`/`history.py` still need an I/O check |
| `summarizer.py` | 231 | Yes. Runs `claude -p` with your Claude login | Only for Claude users |
| `kokoro.py`, `kokoro_provision.py` | 383 | No. Uses kokoro-onnx: a 310 MB model plus a 6 MB voices file, installed in a separate Python 3.12 environment by uv | The logic yes; the install method is wrong for an app installer |
| `chatterbox*.py` plus its requirements file | about 815 | No. Mentioned about 250 times across 13 modules and 8 or more test files | Remove (you already want to) |
| `platform/windows/*` | about 2,500 | No, Windows only. Speech playback uses `winsound` (no real pause), plus global hotkeys, ducking, pausing and the supervisor | The renderer would replace most of it |
| `daemon.py` | 2,934 | Mixed. One `SpeechDaemon` class does everything: messages, the speak loop, summary scheduling, hotkeys, audio, config and the settings page | Needs splitting first |
| `protocol.py`, `transport.py`, `client.py` | about 230 | No. One JSON message per line over local TCP, with a token stored in a lockfile in `~/.sonara` | Any program can connect |

Gaps for a player:
- **No live updates:** `STATUS` returns settings only. Nothing reports what is playing, the queue length, or the paused/muted state.
- **No tab link:** messages carry Claude's session id but nothing that ties them to a terminal tab.
- **No plain "speak this text" message:** the only input is Claude's hook events (`PROSE` deltas and so on).
- **Controls already exist:** `PAUSE`, `SKIP`, `STOP`, `MUTE`, `NAV prev/next`, `REPEAT`, `SET_VOLUME` and `SET_RATE` cover the player buttons.

## 3. Integration options

| Option | Good | Bad |
|---|---|---|
| **A. Use the Sonara daemon over its existing protocol** (Electron connects using the `~/.sonara/lock` token; the player is a React pill) | Fastest. Reuses everything. Needs only an event feed and a tab id added to Sonara | Can't ship to other users: it needs Python, uv, the Sonara plugin and its hooks. Audio stays in the Python process, and `winsound` has no true pause |
| **B. Load Sonara as a library** | Not possible: Electron can't import Python | – |
| **C. Ship a frozen Sonara program (PyInstaller) as an optional download** | Reuses all the logic; follows the dictation "separate program" pattern | Over 200 MB (CPython, onnxruntime, numpy). Python and TypeScript settings and state are duplicated. Still uses `winsound`. You'd be shipping a large unsigned exe |
| **D. Port the core to TypeScript in PrismTerminal's `core/`** | Fits PrismTerminal's rules. Playback in the app gives real pause/seek/volume and the player state for free. One download on opt-in, same as dictation | About 1,500 lines of Python to port (with tests). Two versions of the logic to keep in step |
| **E. Pull out a "sonara-core" package with a stable API** | Clean on paper | Only useful if both apps share one language. In practice it means a written spec plus shared test fixtures, not one shared package |

**Where the text to speak comes from:**
- **Reading terminal output:** poor choice. Claude redraws its screen with escape codes, spinners and partial repaints, and the existing question detection shows how fragile screen-matching is. Only reasonable for plain shell output.
- **Claude Code hooks:** the most detail (streamed text, questions, permissions), but Claude only. It needs a plugin or edits to the user's `settings.json`, which is intrusive for "all users".
- **Following the saved session files** (`~/.claude/projects/*/*.jsonl`, `~/.codex/sessions`): needs no setup, works for Claude and Codex, and PrismTerminal already finds the files and knows which tab runs which agent. You get whole messages rather than a live stream. **Recommended as the main source**, with hooks as an optional extra.
- **Tab id:** to tie hook or file events to a tab, set a `PRISM_TAB_ID` variable in `ptyEnv` and have the hook pass it along.

**Shipping Kokoro to other users:**
- **Today:** Sonara builds a separate Python 3.12 environment with uv and downloads a 310 MB model. That doesn't suit an installer.
- **Options:**
  - **A pinned separate program** such as sherpa-onnx's Kokoro build. This is the same pattern as dictation: official binaries, SHA-256 pins, warm before use.
  - **kokoro-js running on WebAssembly** in a background worker. This avoids a second native module.
  - **Model:** the int8 Kokoro model (about 90 MB, my estimate, not measured) as an opt-in download to a shared `%LOCALAPPDATA%` folder, like the whisper models.
- **Spoken summaries** become an optional extra, only when `claude` is installed.

## 4. Recommended path: option D, with option A as a short prototype

**Phase 0: Sonara cleanup (needed anyway).** Each item is its own PR.
1. Remove Chatterbox: delete three modules, the requirements file and about eight test files, and strip it from `daemon.py`, `tts.py`, `cli.py`, `config.py`, `paths.py`, `webui.py`, `previews.py`, `settings.html`, `kokoro.py` and `assembler.py`. Old config values need to map to a Kokoro voice.
2. Split `daemon.py` into an ingress module (message handling), the speak loop, summary scheduling, audio control and the hotkeys and settings page.
3. Add a `SUBSCRIBE` message that pushes now-playing, queue, paused and muted events. Add a plain `SPEAK` message (text, source, tab).
4. Have `sonara-hook` pass along `PRISM_TAB_ID`.
5. Write the text rules (cleaner and sentence splitting) as shared input-to-output test cases, so a TypeScript port can be checked against them.

**Phase 0 status (2026-10-01): all five prerequisites are done**, in open PRs on Maxaubert/Sonara
awaiting merge (plan: [phase0-plan.md](phase0-plan.md)).
1. Done: Chatterbox removed (#134, PR #147). Saved Chatterbox voices map to `af_heart`; `sonara cleanup` removes the leftovers.
2. Done: `daemon.py` split into the `daemon/` package (#141, PRs #154, #156, #157, #158), with table dispatch and a per-session state registry; the installer moved to `install/` (#142, PR #159). See [architecture.md](../architecture.md).
3. Done: `SUBSCRIBE` state stream and `SPEAK` (#143, PR #163). STATUS returns the same snapshot.
4. Done: the hook passes `SONARA_HOST_TAB` or `PRISM_TAB_ID` as `host_tab` (#143, PR #163).
5. Done: golden text-rule fixtures in `tests/fixtures/text_rules/` (#143, PR #163).

The contract a host can rely on is [docs/protocol.md](../protocol.md). Three facts in this research are now
out of date: the lockfile is `~/.sonara/daemon.lock`; `STATUS` reports now-playing, queue, pause
and mute state; and back/next navigation was removed in #135 (one message, always the last), so
NAV only restarts the current turn and NEXT_SESSION switches sessions.

**Phase 1: prototype on your machine (option A, about 1 PR).**
- A `sonaraBridge` in PrismTerminal's main process connects with the lockfile token and subscribes to events.
- A `<SpeechPlayer>` pill in `core/renderer` (pause, skip, stop, mute, restart, next session, volume) sends the existing protocol messages.
- It's hidden unless the daemon is running. This settles the player design before any porting.

**Phase 2: native audio mode for all users (option D).**
- **Settings:** an "Audio mode" setting in `core/`, off by default, wired through `TermHostConfig` (your call whether Prism gets it too).
- **Text source:** a main-process watcher that follows each agent tab's session file.
- **Ported core:** the cleaner, sentence splitter and router/queue in TypeScript, checked against the Phase 0 test cases.
- **Speech engine:** downloaded with SHA pins on first enable.
- **Playback:** in the renderer, so pause, seek and volume are real. Reuse `mediaPause.ts` for pausing other media.
- **Tests:** a real end-to-end test, like the dictation one.

**Phase 3: hooks and summaries.**
- If Claude Code is installed, an optional mode uses hooks for live streaming plus question and permission announcements.
- Optional `claude -p` summaries.
- The Sonara plugin either stays the "no Prism" way to use it, or becomes a thin client of the Prism engine.

**Decisions for you:**
- Should audio mode go in `core/` (so Prism gets it too)?
- Is whole-message speaking from session files good enough, or is live streaming required?
- sherpa-onnx program or kokoro-js on WebAssembly? This needs a quick test of speed and installer size first.
- What happens to the standalone Sonara plugin long term?

**Files:**
- `C:/Users/Admin/Documents/Claude/Github/Sonara/src/sonara/daemon.py`
- `C:/Users/Admin/Documents/Claude/Github/Sonara/src/sonara/protocol.py`
- `C:/Users/Admin/Documents/Claude/Github/Sonara/src/sonara/hooks_entry.py`
- `C:/Users/Admin/Documents/Claude/Github/Sonara/src/sonara/kokoro.py`
- `C:/Users/Admin/Documents/Claude/Github/PrismTerminal/core/README.md`
- `C:/Users/Admin/Documents/Claude/Github/PrismTerminal/core/main/terminal.ts`
- `C:/Users/Admin/Documents/Claude/Github/PrismTerminal/core/main/dictationEngine.ts`
- `C:/Users/Admin/Documents/Claude/Github/PrismTerminal/core/main/agentResume.ts`
- `C:/Users/Admin/Documents/Claude/Github/PrismTerminal/core/main/mediaPause.ts`
- `C:/Users/Admin/Documents/Claude/Github/PrismTerminal/electron-builder.yml`
