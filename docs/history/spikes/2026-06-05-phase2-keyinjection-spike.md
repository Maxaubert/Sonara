# Phase 2 Feasibility Spike - Key Injection, Numeric Selection, Global Hotkeys

**Date:** 2026-06-05
**Context:** Sonari Phase 2 (`hotkeyd` - global hotkeys + 100% eyes-free option selection).
**Method:** 4 parallel research subagents (Workflow `sonari-phase2-keyinjection-spike`,
run `wf_0bbe041d-50d`) inspecting the installed Claude Code binary (2.1.163, a single
Bun-compiled mach-o; JS bundle in its `__BUN` section, read via `strings`+grep), official
docs, and live compilation/probes on this Mac. Findings were then **reviewed by the
controller** (cross-checked for consistency) and the pivotal claim **empirically
confirmed by the user via a live picker test**.

---

## Headline result

**The highest-risk piece of the whole project - synthetic key injection to drive the
picker - is NOT needed.** Claude Code's pickers accept **native numeric selection**, so
Sonari reads numbered options aloud and the user presses the digit. No CGEventTap
intercept/suppress, no key injection, on the core selection path.

---

## Finding 1 - AskUserQuestion numeric selection - **CONFIRMED (high + live-verified)**

- Pressing a digit `1`–`9` **immediately selects** that option (fires `onChange`, **no
  Enter**). Options render **with visible numbers**. Initial highlight = **first** option.
  Arrows **wrap** at both ends. Also supports up/down, `j`/`k`, `ctrl+n`/`ctrl+p`, Enter.
- **Evidence:** decompiled handler
  `if(_!=="numeric"&&/^[0-9]$/.test(W)){...let R=parseInt(W)-1; ...q.onChange?.(k.value)}`;
  plus **three independent GitHub issues** (#20590 closed *not-planned*, #22300, #25624)
  complaining the behavior is *too* eager (digits fire even in the "Other" free-text box).
- **LIVE TEST (2026-06-05):** user pressed a number at a real AskUserQuestion picker → it
  selected. ✅ Pivotal claim empirically validated on this machine.
- **Caveats:** digit-select is **immediate/irreversible** (mistyped digit commits);
  options **>9** are not digit-selectable (single-char regex) → need arrow fallback;
  digits also fire while in "Other" free-text (can't type a custom answer starting with a
  digit - known unfixed CC bug).
- **Needs live confirmation (build-time):** multiSelect keys (space-toggle? digit-toggle?
  Enter-confirm), multi-question `Tab` movement, "Other" narration.

## Finding 2 - Permission prompt & ExitPlanMode - **high confidence**

- Both use the **same Select component** as AskUserQuestion (`O8`/`H57`), so: **numbered
  list, digit `1`–`9` selects, `Esc` cancels/denies**. Letters (`y`/`n`/`a`/`d`) are **not**
  bound - don't rely on them.
- Permission prompt = multi-option (`Yes` / `Yes, and don't ask again for …` variants /
  `No (esc)`), **dynamic** order/labels. ExitPlanMode = multi-option too (`Yes, auto-accept
  edits` / `Yes, manually approve` / optional `Ultraplan` / `No, keep planning` input);
  empty-plan fallback is plain `Yes`/`No`. Plan text field = **`plan`** (injected from disk
  via `normalizeToolInput`); `checkPermissions` → `{behavior:"ask", message:"Exit plan
  mode?"}`. `Shift+Tab` = approve-with-auto-accept; `Ctrl+G` = edit plan in editor.
- **Design rule:** never hard-code "press 2 = …" (options are dynamic). **Read the rendered
  numbered list aloud; safe constants are `1`=first/Yes and `Esc`=deny.**
- **Open:** does the `Notification` `permission_prompt` hook payload actually carry the
  option list? If not, Sonari reads the action + a generic cue and relies on `1`/`Esc`.
  → capture a golden payload during the build.

## Finding 3 - macOS key injection - **feasible but DEFERRED (not needed for v1)**

- CGEventPost (Quartz) works; recommended impl would be a signed Swift binary (pyobjc/pynput
  are **absent** on this Mac → would add pip deps). Needs **Accessibility** (not Input
  Monitoring).
- **Blocker that justifies avoiding it:** **Secure Event Input** silently swallows injected
  keys - and it was **already ON** during the probe (`secure=true`). An injection-based
  design would fail silently for a blind user. Native numeric selection sidesteps this
  entirely.
- PoC saved (`sonari_inject_poc.swift`) for the *future* path only; **not built in Phase 2.**

## Finding 4 - Global hotkeys - **CONFIRMED (high; compiled & ran on this Mac)**

- Use **Carbon `RegisterEventHotKey`** in a small Swift binary. It fires system-wide while
  the terminal is frontmost, **consumes only the registered combo** (all other typing passes
  through), and **requires no Accessibility / Input-Monitoring permission** (narrowly scoped).
  Compiled with `/usr/bin/swiftc`, registered `Ctrl+Opt+S`, stayed alive in run loop, no
  permission prompt.
- **Rejected:** `CGEventTap` (needs Input Monitoring; brittle "silent disable race");
  `NSEvent` global monitor (observe-only, can't consume, needs Accessibility).
- LaunchAgent must run in the **Aqua (GUI) session**; `NSApplication.run` +
  `.accessory` policy (no Dock icon).
- **Controller review caught:** the agent's proposed `Ctrl+Opt+<letter>` default **collides
  with the VoiceOver modifier** - bad for an accessibility tool. **Default changed to
  `Ctrl+Cmd`** (user-approved 2026-06-05).
- **Open:** confirm a hotkey fires AND leaks no character / no beep while a terminal is
  focused (needs an interactive GUI session); confirm LaunchAgent-launched binary registers
  identically.

---

## Decisions (locked for Phase 2)

1. **Selection = native numeric, no injection.** Read numbered options + "press the number /
   Esc to cancel" cue + immediate-select warning; cache last picker for re-read.
2. **`hotkeyd` = Swift + Carbon `RegisterEventHotKey`**, LaunchAgent `com.sonari.hotkeyd`,
   talks to `speechd` over the existing Unix socket. **No macOS permission required.**
3. **Default modifier = `Ctrl+Cmd`** (avoids VoiceOver's `Ctrl+Opt`); fully rebindable.
4. **Drop** key injection + arrow-index tracking from v1. Keep the inject PoC for a future
   "read text selection" feature (the only thing needing Accessibility) - deferred.
5. **Signing:** ad-hoc for local install now; Developer ID + notarization at Phase 3 ship.

## Must validate live during the build (per "always-validate-and-review")

- multiSelect digit/space + Enter; multi-question `Tab`; `>9`-option arrow fallback.
- Whether `permission_prompt` payload carries options (golden payload).
- Each hotkey fires without leaking a keystroke; LaunchAgent parity.

## Artifacts

- `sonari-hotkeyd-poc.swift` - Carbon RegisterEventHotKey PoC (basis for `hotkeyd`).
- `sonari_inject_poc.swift` - CGEventPost injection PoC with Accessibility/Secure-Input/
  frontmost guards (**reference only; deferred feature**).
