# Echo — Eyes-Free Claude Code (Design Spec)

**Status:** Draft for review
**Date:** 2026-06-04
**Working name:** Echo (final name TBD)
**Replaces:** the existing `claude-tts` plugin (PTY wrapper + transcript-polling hooks)
**Platform:** macOS, Claude Code 2.1.162+

---

## 1. Purpose & success criteria

A blind / low-vision developer must be able to use Claude Code **without looking at the
screen at all** — both to *hear* everything important and to *act* on it (answer
questions, approve actions, choose plans) entirely by keyboard and voice output.

The current `claude-tts` tool falls short: speech interrupts itself, interactive
options are never read, and the experience is generally unreliable. This is a **ground-up
redesign**, not a patch.

**Success = the user can run a full Claude Code session — including planning, answering
multiple-choice questions, and approving tool actions — with the screen off.**

### Primary user
- Has some residual vision, uses a magnifier, is not a fluent screen-reader user.
- Decision (confirmed): the product must **not rely on residual vision** for any step.

### Confirmed product decisions
- **Voice:** macOS *enhanced/neural* voice via `say -v` (free, instant, offline).
- **Control:** global keyboard hotkeys (via a small background helper; macOS Accessibility
  permission approved by the user).
- **Default verbosity:** "Everything relevant," switchable live.
- **Concurrency:** the user *sometimes* runs multiple sessions → per-session isolation is
  mandatory.
- **Selection:** 100% eyes-free (no looking required to pick options / approve plans).
- **Ordering:** speech is **strictly in order**; decisions are alerted instantly by a
  **distinct earcon per type** but never spoken out of order (see §4).

### Non-goals (v1)
- Non-macOS platforms (no Linux/Windows; `say` is macOS-only).
- Cloud / neural-API TTS (latency + cost). Architecture leaves room to add later.
- Speech *input* / dictation as a control method (keyboard hotkeys only for now).
- Reading every low-level detail by default (verbosity-gated).

---

## 2. Why the current tool fails (root causes, evidence-based)

From a full audit of the existing repo:

1. **Two uncoordinated narration engines.** A PTY wrapper (`bin/claude-speak`) streams
   prose to `say` from raw terminal bytes, while the hooks (`pre-tool-use.sh`, `stop.sh`)
   *independently* re-read the same prose from the transcript and also call `say`. No
   shared queue → double-speaking and mutual cut-off.
2. **"Interruption" = a sledgehammer.** Every new utterance runs `pkill -x say`, killing
   **all** speech on the machine (including other sessions) and starting over. There is no
   cross-process queue. *This is the "new speech interrupts ongoing speech" bug.*
3. **Options are structurally invisible.** The text extractor keeps only
   `type == "text"` content blocks and discards `tool_use` blocks. `AskUserQuestion`
   choices and `ExitPlanMode` plans **are** `tool_use` blocks, so they never reach speech.
4. **Per-session races.** Hooks pick the transcript by newest file mtime (wrong session
   under concurrency) and share a single unlocked cursor file; the `Stop` hook can drop the
   final turn; `PreToolUse` speaks synchronously and stalls tool execution.

The redesign eliminates each of these by construction.

---

## 3. Platform capabilities (verified against the installed binary, not just docs)

Confirmed present in `~/.local/share/claude/versions/2.1.162`:

- **`MessageDisplay`** hook with a streaming **`delta`** field → real-time prose narration
  **through a hook**. This lets us **delete the entire PTY wrapper and the `claude` alias.**
- **`PreToolUse`** fires for **`AskUserQuestion`** and **`ExitPlanMode`** with their full
  `tool_input` *before* the picker renders → we can read options/plans as structured data.
- **`Notification`** hook with **`idle_prompt`** and **`permission_prompt`** matchers →
  "ready for you" cue and spoken permission prompts.
- Every hook receives **`session_id`** + **`transcript_path`** on stdin → real per-session
  scoping (no mtime guessing).
- Keybindings map only to **built-in actions** (`select:next`, `confirm:yes`, …) and pickers
  use **arrows/Enter** by default → Claude Code's own keybindings **cannot** trigger our
  tool, which is *why* we need an OS-level hotkey helper.

### Items to verify empirically during implementation ("verification list", captured as "golden payloads")
These do not change the architecture but pin down exact field names / behavior:
1. Exact `MessageDisplay` payload (`delta`/`index`/`final`), batching cadence, and whether
   it blocks rendering (keep the hook <100 ms regardless).
2. `AskUserQuestion` `tool_input` schema (questions, headers, option labels/descriptions,
   `multiSelect`, "Other"/free-text path).
3. `ExitPlanMode` `tool_input` plan field name **and** how its approval dialog is shaped
   (yes/no confirm vs. multi-option select).
4. **Native numeric selection**: do permission prompts and `AskUserQuestion` accept `1/2/3`
   key presses to pick option N? (Permission prompts appear to show numbered options.) If
   yes, selection becomes trivial (see §6).
5. Picker starting highlight index and whether arrows wrap.
6. `Notification` payloads for `permission_prompt` / `idle_prompt`.
7. CGEventTap intercept+suppress and synthetic key injection reliability into common
   terminals (Terminal.app, iTerm2, VS Code integrated terminal).

**Verification method:** a logging hook dumps each event's raw stdin to a file; we run a
real Claude session that hits each case, then drive tests from the captured payloads.

---

## 4. The ordering model (the core UX contract)

This is the heart of the design and directly addresses the "voice falls behind the screen"
concern.

**Principle: the voice never jumps ahead of you.**

- **Spoken content is strictly FIFO.** A permission, question, or plan is spoken *in its
  natural place* — after the prose that explains it. If the voice is on message 3 and a
  permission appears after message 5, you hear **3 → 4 → 5 → the permission.** Context
  always precedes the decision.
- **Alerts are instant and separate from speech.** The moment a decision appears, a short
  **distinct earcon** plays immediately (different sound for permission / choice / plan /
  error / turn-done / ready). The *alert* barges in; the *spoken detail* does not.
- **No time pressure.** When Claude hits a permission / question / plan it is **blocked**
  until you respond — the prompt waits indefinitely — so hearing context first costs
  nothing but the seconds to listen.
- **You hold the pace** (hotkeys, §6): *skip* (next item), *jump to the decision* (skip
  queued prose, go straight to the pending question/permission — an explicit choice to
  forgo context), *catch up to live* (flush backlog, resume at newest), *faster/slower*.
- **Auto-flush only on your action.** Submitting a new prompt or pressing stop clears the
  queue. The system never silently skips context on its own.

"Higher priority" therefore means **"alert you instantly with a sound,"** never **"speak it
out of order."**

---

## 5. Architecture

Three parts: **two long-lived singletons + thin hook clients.**

```
 Claude Code session(s)
        │  (hook events on stdin: MessageDisplay, PreToolUse, Notification, Stop, …)
        ▼
 ┌──────────────┐   tiny JSON over Unix socket   ┌───────────────────────────┐
 │  hook clients │ ─────────────────────────────▶ │         speechd           │
 │ (return <10ms)│                                │  (the speech daemon)      │
 └──────────────┘                                │  • single FIFO speech queue│
                                                  │  • one `say` child (kill   │
 ┌──────────────┐   control + picker state        │    only its own PID)      │
 │   hotkeyd     │ ◀───────────────────────────▶  │  • earcon channel (afplay)│
 │ (global keys, │     JSON over Unix socket       │  • per-session + fg model │
 │  key inject)  │                                │  • config: voice/rate/verb │
 └──────────────┘                                └───────────────────────────┘
```

State/config/sockets live under `~/.echo/` (sock, `config.json`, per-session state, logs).

### 5.1 `speechd` — the speech daemon (singleton, machine-wide)
- Owns the **only** audio output and a **single FIFO speech queue**. Everything else just
  sends it messages.
- Runs **one** `say` subprocess at a time and tracks its PID; "stop" terminates **that
  child only** — never `pkill -x say`. (Fixes root cause #2.)
- **Earcon channel** is independent of the speech queue: earcons play immediately via
  `afplay` (short sound files; default to `/System/Library/Sounds/*`), and can overlap or
  briefly duck speech. (Implements §4 instant alerts.)
- **Session-aware / foreground model:** tracks `foreground_session_id` (updated on
  `UserPromptSubmit` and `SessionStart`). Messages are tagged with `session_id`. Only the
  **foreground** session is spoken; background sessions are **earcon-only** for decisions
  (configurable). (Fixes root causes #1 and #4; handles "sometimes multiple sessions.")
- **Prose assembly:** `MessageDisplay` deltas are cleaned (markdown → speech), assembled
  into sentence-sized chunks, deduped by `index`, and enqueued in order.
- **Config** (persisted, live-editable): voice, rate, verbosity level, background-session
  policy, earcon set.
- **Lifecycle:** started by the `SessionStart` hook (or lazily by the first hook that needs
  it) via a macOS LaunchAgent so it survives across sessions; idle-exits after no sessions.

**Speech queue operations** (driven by hooks and hotkeyd):
`enqueue(item)`, `skip_current()`, `jump_to(predicate=decision)`, `catch_up()` (clear all),
`flush(session)`, `repeat_last()`, `set_rate()`, `set_verbosity()`.

### 5.2 Hooks — thin clients (declared in `hooks/hooks.json`)
Each hook reads stdin JSON, extracts the minimum, sends one socket message, and exits fast.
If the daemon is down, it auto-starts it. (MessageDisplay has a ~10 s timeout but we target
<100 ms so rendering is never stalled.)

| Event (matcher) | What it sends to `speechd` |
|---|---|
| `MessageDisplay` | `{prose, session, delta, index, final}` — queued FIFO |
| `PreToolUse` · `AskUserQuestion` | parsed `{choice, questions[], options[], multiSelect}` + opens **picker mode** |
| `PreToolUse` · `ExitPlanMode` | `{plan, text}` (+ picker mode if its approval is multi-option) |
| `PreToolUse` · other tools | brief `{tool_announce}` — only at higher verbosity |
| `Notification` · `permission_prompt` | `{permission, action, options[]}` + earcon (+ picker mode only if multi-option) |
| `Notification` · `idle_prompt` | `{ready}` earcon |
| `Stop` | `{turn_done}` earcon; reconcile any unspoken final text from `transcript_path` |
| `UserPromptSubmit` | `{flush, session}` + set foreground |
| `SessionStart` | ensure daemon up; `{session_start, session}`; set foreground |
| `SessionEnd` | `{session_end}` cleanup |

All decision events (`choice`/`plan`/`permission`/`error`) trigger their **distinct earcon
immediately**, while the spoken detail enters the queue in order (§4).

### 5.3 `hotkeyd` — the global-hotkey helper (singleton)
Long-lived; connects to `speechd`; runs via LaunchAgent; requires macOS Accessibility /
Input-Monitoring permission. Two jobs:

**(a) Speech control — works anywhere, even mid-speech:**
stop · repeat-last · skip · jump-to-decision · catch-up-to-live · faster · slower ·
cycle-verbosity · re-read-options · read-code-block-in-full.
Default keymap shipped; user-overridable via `~/.echo/keymap.json`.

**(b) Picker mode — 100% eyes-free selection.**
When `speechd` signals a picker is open, it passes `hotkeyd` the option structure (count,
labels, `multiSelect`, number of sub-questions). Because the option list came from the hook
payload, `hotkeyd` knows the choices **without reading the screen**. Two selection paths,
chosen by what the §3 verification list (item 4, native numeric selection) finds:

- **If native numeric selection works** (preferred): we read options *with numbers*; the
  user presses the number; `hotkeyd` simply confirms it aloud. Trivial and robust — no key
  injection.
- **If not:** `hotkeyd` intercepts the keys and drives the native picker itself:
  - **digit N** → inject the right count of Up/Down + Enter to select option N.
  - **Up/Down** → update its tracked index, inject the arrow, speak the option landed on.
  - **multiSelect** → space toggles ("option N selected/deselected"), Enter confirms.
  - **multiple sub-questions** → Tab between fields (`confirm:nextField`), track field index.
  - **"Other"/free-text** → switch to passthrough; "type your answer, Enter to submit."
  `hotkeyd` is the *sole* source of navigation while a picker is open, so its tracked index
  stays in sync with the highlight (assume start index 0; verify; offer "reset to top").

> Permission and plan-confirm dialogs that are simple yes/no need **no** injection — once
> the prompt is read aloud, single-key `y`/`n`/Enter is already eyes-free. Picker mode is
> only needed for multi-option lists.

**Risk note:** intercept-and-suppress + synthetic key injection is the highest-risk piece.
P2 therefore *starts* with a feasibility spike (§8) before the full picker logic is built.

---

## 6. Voice, verbosity, content handling

- **Voice:** an enhanced/neural macOS voice via `say -v "<Voice>" -r <rate>`. Install flow
  prompts the user to download one (System Settings → Accessibility → Spoken Content →
  System Voice → manage voices) and `doctor` verifies it's present.
- **Verbosity (live-switchable):**
  - *Everything* — prose + options/plans/permissions + brief tool announcements + errors +
    code summaries.
  - *Medium* — prose + decisions + errors; skip routine tool announcements.
  - *Quiet* — prose + decisions only.
  - Earcons fire in all levels.
- **Markdown → speech:** reuse/improve the existing cleaner (code fences, inline code,
  headings, bold, links → "link", tables stripped, whitespace collapsed).
- **Code blocks:** summarized as "*N-line `<lang>` block*" with a hotkey to read in full.
- **Errors:** spoken (distinct earcon) with the message; depth verbosity-gated.

---

## 7. Packaging, install, migration

- Ship as a **real Claude Code plugin**: `.claude-plugin/plugin.json` + `hooks/hooks.json`
  (declarative, using `${CLAUDE_PLUGIN_ROOT}`) + namespaced `commands/` (e.g. `/echo:status`,
  `/echo:verbosity`, `/echo:voice`, `/echo:repeat`, `/echo:stop`, `/echo:doctor`). No more
  hand-editing `settings.json`.
- **Hotkeyd + speechd** installed as **LaunchAgents**; install requests the Accessibility
  permission and guides the voice download.
- **Slash commands are a fallback control surface;** global hotkeys are primary.
- **`doctor`** checks: enhanced voice present? Accessibility granted? `speechd` up?
  `hotkeyd` up? plugin hooks registered? socket reachable?
- **Migration from old `claude-tts`:** an uninstaller removes the `claude`→`claude-speak`
  alias and the `~/.zshrc` PATH edits, the three old hooks from `settings.json`, the old
  `bin/` scripts, and `~/.claude-tts-enabled` / `~/.claude-tts-pos`. The new design uses
  **no alias and no shell-rc edits.**

---

## 8. Build plan (phased, each independently verifiable)

**Phase 1 — Output (hear everything; fixes all three current bugs).**
`speechd` (queue, single `say` child, earcon channel, per-session/foreground) + hooks
(`MessageDisplay`, `Notification`, `Stop`, `UserPromptSubmit`, `SessionStart`) + voice +
verbosity + slash commands + plugin packaging + migration/uninstall.
*Exit criteria:* a real session narrates prose in order, plays the right earcons, reads
permission prompts and (read-only) options/plans, never double-speaks, never `pkill`s
system-wide, and is correctly scoped to the foreground session.

**Phase 2 — Control & selection (100% eyes-free + keyboard control).**
*Starts with a spike:* confirm native numeric selection (§3 verification list, item 4) and, if needed, validate
CGEventTap intercept/suppress + key injection in Terminal/iTerm/VS Code. Then build
`hotkeyd`: speech-control hotkeys + picker mode (numeric path and/or injection path,
multiSelect, multi-question, "Other").
*Exit criteria:* the user picks any AskUserQuestion option, approves/denies permissions, and
accepts/rejects plans with the screen off.

**Phase 3 — Polish & ship.**
Earcon set + author/select sounds; robust edge cases (rate-limit/StopFailure, sidechains,
long backlogs, picker desync recovery); background-session policy refinements; docs &
onboarding; marketplace packaging.

---

## 9. Testing strategy

- **Deterministic pipeline tests:** mock `say`/`afplay` to *record calls instead of playing
  audio*; feed recorded hook payloads through the real hooks → `speechd`; assert the exact
  spoken/earcon **sequence**. This covers ordering (§4), dedup, verbosity filtering,
  foreground gating, skip/jump/catch-up/flush — all without sound.
- **Golden payloads:** capture real `MessageDisplay` / `PreToolUse(AskUserQuestion,
  ExitPlanMode)` / `Notification` stdin from a live session; use as test fixtures and to pin
  exact schemas (§3 verification list).
- **`hotkeyd` picker mode:** a simulation harness for index tracking + the injection plan;
  plus a scripted **manual** test checklist across terminals (hardest to fully automate).
- **TDD throughout** per the team's standard workflow; `doctor` as an always-on smoke test.

---

## 10. Open questions / decisions deferred to planning
- Implementation language per component (likely Python 3 for `speechd`/hooks for
  consistency + no build step; `hotkeyd` may be Python+Quartz or a small Swift CGEventTap
  binary — decided by the P2 spike).
- Final earcon sound set (system sounds vs. custom).
- Exact default keymap for `hotkeyd`.
- Final product name.
```