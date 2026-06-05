# Sonari Phase 2 — Control & Selection (Design Spec)

**Status:** Approved (user, 2026-06-05) — ready for implementation planning
**Date:** 2026-06-05
**Supersedes:** §5.3 (`hotkeyd`) of `2026-06-04-echo-eyes-free-claude-code-design.md`
**Depends on:** Phase 1 (complete) — `speechd`, hooks, the Unix socket protocol.
**Spike:** `../spikes/2026-06-05-phase2-keyinjection-spike.md` (read first).

---

## 1. Goal

Close the eyes-free loop: the user **answers questions, approves/denies permissions,
accepts/rejects plans, and controls speech** entirely by keyboard, screen off.

**Exit criteria:** with the screen off, the user can (a) hear a picker's numbered options
and select any one, (b) approve/deny a permission, (c) accept/reject a plan, and (d) drive
speech (stop / repeat / skip / jump-to-decision / catch-up / faster / slower / verbosity /
re-read-options) via global hotkeys — all without the speech pipeline regressing.

## 2. Key spike outcomes that shape this design

1. **Native numeric selection works** (live-verified): Claude Code's pickers
   (AskUserQuestion, permission, ExitPlanMode) render **numbered** options and a digit
   `1`–`9` selects instantly; `Esc` denies/cancels. **⇒ No synthetic key injection.**
2. **Global hotkeys** via Carbon **`RegisterEventHotKey`** fire while a terminal is focused,
   consume only the registered combo, and need **no macOS permission**.
3. Therefore Phase 2 needs **zero special permissions** and **no event interception** — a
   massive de-risking versus the original §5.3.

## 3. Architecture

Two additions to the Phase 1 system; **the Phase 1 pipeline is untouched.**

```
  Global keypress (Ctrl+Cmd+X, any app focused)
        │
        ▼
  ┌─────────────┐   newline-JSON over ~/.sonari/speechd.sock   ┌──────────┐
  │  hotkeyd     │ ───────────────────────────────────────────▶ │ speechd  │
  │ (Swift,      │     {"type":"stop"} / {"type":"reread_..."}   │ (Phase 1)│
  │  Carbon hk)  │                                               └──────────┘
  └─────────────┘
        ▲ reads ~/.sonari/keymap.json (defaults shipped, user-rebindable)

  Selection: NO new component. speechd already speaks numbered options from the
  PreToolUse/Notification hook payloads; the user presses the digit natively.
```

### 3.1 `hotkeyd` — global hotkey helper (new, Swift)

- **Mechanism:** `RegisterEventHotKey` + `InstallEventHandler(kEventClassKeyboard /
  kEventHotKeyPressed)`; `NSApplication.run` with `setActivationPolicy(.accessory)` (no Dock
  icon). Basis: `spikes/sonari-hotkeyd-poc.swift` (compiled & ran on this Mac).
- **No permission required** (narrowly scoped) — do not request Accessibility/Input
  Monitoring.
- **Keymap:** loads `~/.sonari/keymap.json`; if absent, writes the shipped default. Each
  entry maps an **action** to `{ "key": "<name>", "mods": ["ctrl","cmd"] }`. hotkeyd resolves
  key names → virtual key codes and mod names → Carbon masks. On registration failure for a
  combo (already claimed), it logs/announces that one and continues with the rest.
- **On fire:** connect to `~/.sonari/speechd.sock`, write the action's protocol message
  (one newline-delimited JSON line, see §4), close. Fire-and-forget; if the socket is
  absent, it attempts to start `speechd` via the existing client/ensure path, else no-ops.
- **Lifecycle:** LaunchAgent `com.sonari.hotkeyd` in the **Aqua** (GUI) session,
  `RunAtLoad=true`, `KeepAlive=true`.
- **Debug/CLI modes** (for testing without pressing keys): `--list` prints the resolved
  keymap; `--print <action>` prints the exact JSON line it would send (no socket);
  `--once <action>` sends that action's message once and exits.
- **Default keymap** (modifier = `Ctrl+Cmd`, chosen to avoid VoiceOver's `Ctrl+Opt`):

  | Combo | action | message → speechd |
  |---|---|---|
  | `Ctrl+Cmd+S` | `stop` | `{"type":"stop"}` |
  | `Ctrl+Cmd+R` | `repeat` | `{"type":"repeat"}` |
  | `Ctrl+Cmd+.` | `skip` | `{"type":"skip"}` |
  | `Ctrl+Cmd+D` | `jump_decision` | `{"type":"jump_decision"}` |
  | `Ctrl+Cmd+L` | `catch_up` | `{"type":"catch_up"}` |
  | `Ctrl+Cmd+]` | `faster` | `{"type":"set_rate","delta":25}` |
  | `Ctrl+Cmd+[` | `slower` | `{"type":"set_rate","delta":-25}` |
  | `Ctrl+Cmd+V` | `cycle_verbosity` | `{"type":"cycle_verbosity"}` |
  | `Ctrl+Cmd+O` | `reread_options` | `{"type":"reread_options"}` |

### 3.2 `speechd` additions (small, additive — `src/sonari/`)

All existing control ops (`stop`, `skip`, `repeat`, `jump_decision`, `catch_up`,
`set_verbosity`, `set_voice`, `status`, `ping`) already exist. Add:

1. **`reread_options`** (new `MsgType.REREAD_OPTIONS`): speechd **caches the last picker's
   spoken option text** per foreground session whenever it handles `CHOICE` / `PLAN` /
   `PERMISSION`. On `reread_options`, it re-enqueues that cached text (or speaks "no options
   to repeat" if none). Cache is cleared on `flush` / `session_end`.
2. **Relative rate** on `MsgType.SET_RATE`: if the message carries `delta`, set
   `rate = clamp(current + delta, RATE_MIN, RATE_MAX)` (defaults: 100–400 wpm, step 25);
   absolute `rate` still supported. speechd speaks a terse confirmation ("rate 225").
3. **`cycle_verbosity`** (new `MsgType.CYCLE_VERBOSITY`): advance
   `everything → medium → quiet → everything`, persist, and speak the new level name.
4. **Selection cue + warning** in the `CHOICE` / `PERMISSION` / `PLAN` narration: after the
   numbered options, append a **terse, verbosity-gated** cue — at `everything`: *"Press the
   option's number to choose, or Escape to cancel."*; at `medium`/`quiet`: omit (the user
   already knows). The first picker per session also appends *"Selecting is immediate."* once.
   Implemented in the narration builder, not the hooks.

### 3.3 Data flow (selection, end to end — no new code on the hot path)

`PreToolUse(AskUserQuestion)` → hook sends `CHOICE{questions,options}` → speechd speaks
"Question … Option 1: … Option 2: … Press the number, or Escape." + caches it → **user
presses the digit** (native) → Claude Code selects. `Ctrl+Cmd+O` re-speaks the cache.
Permissions/plans follow the same shape (read the rendered numbered options; `1`/`Esc`).

## 4. Protocol changes (`src/sonari/protocol.py`)

Add to `MsgType`: `REREAD_OPTIONS = "reread_options"`, `CYCLE_VERBOSITY =
"cycle_verbosity"`. `SET_RATE` gains an optional `delta` field. No version bump needed
(additive); keep `PROTOCOL_VERSION = 1`.

## 5. Packaging / install / doctor

- `sonari install`: build `hotkeyd` (`swiftc`, **ad-hoc signed** for local), place the
  binary, write the `com.sonari.hotkeyd` LaunchAgent, write default `~/.sonari/keymap.json`
  if absent, and `launchctl bootstrap` it. (Public-release Developer-ID signing/notarization
  is deferred to Phase 3.)
- `sonari uninstall`: bootout + remove the LaunchAgent and binary; leave `keymap.json`.
- `sonari doctor`: add checks — `swiftc` present? `hotkeyd` binary built? LaunchAgent
  loaded/running? keymap parses? socket reachable from a test send? Report each.
- New slash command `/sonari:keymap` prints the active keymap (calls `hotkeyd --list`).

## 6. Error handling & edge cases

- **>9 options:** digits `1`–`9` select; options `10+` need arrows. speechd appends *"More
  than nine options; use arrow keys for ten and up."* when count > 9.
- **multiSelect:** narration says *"Select multiple: press each number (or Space on the
  highlighted item), then Enter to confirm."* — **exact keys verified live during the build**
  (open question O-1).
- **multi-question AskUserQuestion:** narration notes *"Tab moves to the next question."* —
  verified live (O-2).
- **"Other"/free-text option:** narrated as *"Option N, Other: type your answer."*; note that
  digits also type into it (CC bug) so a custom answer starting with a digit isn't possible
  via the picker.
- **permission_prompt payload may lack the option list (O-3):** if options are present, read
  them numbered; if not, read the action + *"Choose the numbered option, or Escape to
  deny."* and rely on `1`=proceed / `Esc`=deny.
- **hotkey registration conflict:** announce which combo failed; the rest still work; user
  rebinds in `keymap.json`.
- **Secure Event Input / injection:** N/A — Phase 2 injects nothing.

## 7. Testing strategy

- **Deterministic (Python, existing mock-`say`/`afplay` harness):** `reread_options` caches
  then re-speaks the right text and "nothing to repeat" when empty; cache cleared on
  flush/session_end; `set_rate` delta clamps at bounds and speaks confirmation;
  `cycle_verbosity` cycles + persists + announces; selection cue appears only at the right
  verbosity and the once-per-session immediate-select warning fires once; `>9` note appears.
- **`hotkeyd` logic (no GUI):** test keymap parsing (key/mod name → code/mask), the
  action→JSON mapping (via `--print <action>` golden strings), and that `--once <action>`
  writes the exact bytes to a stub socket. The Carbon registration itself is covered by the
  manual checklist (it was validated by the spike PoC).
- **Live smoke checklist (manual, screen-off):** each hotkey produces the right speechd
  reaction; numeric selection on a real AskUserQuestion / permission / plan; multiSelect;
  multi-question Tab; `>9` options; `Ctrl+Cmd+O` re-read; verify no keystroke leaks/beeps
  when a hotkey fires in Terminal/iTerm/VS Code.
- **`doctor`** as an always-on smoke test. **TDD throughout** (subagent-driven).

## 8. Open questions to resolve empirically during the build

- **O-1** multiSelect exact keys (digit-toggle vs Space-toggle; Enter to confirm).
- **O-2** multi-question: does a digit select within the current sub-question, and does Tab
  advance vs submit?
- **O-3** does the `Notification permission_prompt` hook payload include the option list?
- **O-4** confirm a hotkey leaks no character/beep in each terminal, and the
  LaunchAgent-launched `hotkeyd` registers identically to a shell-launched one.

## 9. Out of scope (deferred)

- Synthetic key **injection** and arrow-index tracking (obviated by native numeric; PoC kept
  at `spikes/sonari_inject_poc.swift` for a future feature).
- **"Read/act on text selection"** (the only feature needing Accessibility) — later.
- Developer-ID signing + notarization + PyPI/marketplace publish — **Phase 3**.
