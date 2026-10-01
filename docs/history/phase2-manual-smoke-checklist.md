# Sonari Phase 2 - Manual Smoke Checklist (screen-off, live)

Run these on the real Mac after `sonari install`. The deterministic pytest suite covers
daemon/keymap/cli logic and Swift compilation; this covers the **Carbon hotkey runtime**
and **Claude Code picker behavior**, which cannot be unit-tested. It also resolves spec
open questions **O-1..O-4**
(`docs/superpowers/specs/2026-06-05-sonari-phase2-control-selection-design.md` §8).

Each item is a checkbox with the exact action to perform and the expected result. Do these
with the **screen off** wherever a `(screen off)` tag appears - that is the real test.

---

## Setup

- [ ] **Install.** Run `sonari install`. Expect: it builds the hotkeyd binary, writes the
  default keymap + resolved JSON, and loads both LaunchAgents with no error.
- [ ] **Doctor is all-ok.** Run `sonari doctor`. Expect: every line starts `[ok ]`,
  including `swiftc`, `hotkeyd binary`, `hotkeyd resolved keymap`, and `keymap resolves`.
- [ ] **Keymap prints 9 actions.** Run `sonari keymap`. Expect exactly:
  stop=Ctrl+Cmd+S, repeat=Ctrl+Cmd+R, skip=Ctrl+Cmd+., jump_decision=Ctrl+Cmd+D,
  catch_up=Ctrl+Cmd+L, faster=Ctrl+Cmd+], slower=Ctrl+Cmd+[, cycle_verbosity=Ctrl+Cmd+V,
  reread_options=Ctrl+Cmd+O.
- [ ] **hotkey daemon is running.** Run `launchctl list | grep com.sonari.hotkeyd`. Expect:
  one line with a numeric PID in the first column (not `-`) and exit status `0`.
- [ ] **speechd is running.** Run `launchctl list | grep com.sonari.speechd`. Expect a
  running PID. (Hotkeys do nothing audible if speechd is down.)

---

## O-4 - each of the 9 hotkeys fires, leaks no character, makes no beep

Do this in **each** terminal: **Terminal.app**, **iTerm2**, **VS Code integrated
terminal**. Sit at an interactive shell prompt with an empty command line (just a cursor).

For every combo below: press it, then look at the command line and listen.
**Expected for ALL:** the command line stays empty (no character/control char inserted),
the system makes **no beep**, and Sonari reacts as noted. (No active speech is fine for
some actions when nothing is queued; the key point is no leak/beep.)

### Terminal.app
- [ ] **Ctrl+Cmd+S (stop)** - empty line, no beep. While Sonari is mid-utterance, this
  silences it immediately.
- [ ] **Ctrl+Cmd+R (repeat)** - empty line, no beep. Re-speaks the last utterance.
- [ ] **Ctrl+Cmd+. (skip)** - empty line, no beep. Drops the current utterance, moves on.
- [ ] **Ctrl+Cmd+D (jump_decision)** - empty line, no beep. Jumps to the pending decision.
- [ ] **Ctrl+Cmd+L (catch_up)** - empty line, no beep. Drains/catches up the queue.
- [ ] **Ctrl+Cmd+] (faster)** - empty line, no beep. Speaks "Rate N." with N higher.
- [ ] **Ctrl+Cmd+[ (slower)** - empty line, no beep. Speaks "Rate N." with N lower.
- [ ] **Ctrl+Cmd+V (cycle_verbosity)** - empty line, no beep. Speaks "Verbosity <level>."
- [ ] **Ctrl+Cmd+O (reread_options)** - empty line, no beep. Re-speaks the last picker, or
  "No options to repeat." if none.

### iTerm2
- [ ] Repeat all 9 combos in iTerm2 - same expectation (empty line, no beep, right
  reaction). Note any combo iTerm2 swallows or beeps on.

### VS Code integrated terminal
- [ ] Repeat all 9 combos in the VS Code integrated terminal - same expectation. VS Code
  has many built-in Ctrl/Cmd bindings; note any combo VS Code intercepts before hotkeyd.

### Rate / verbosity behavior spot-check
- [ ] Press **Ctrl+Cmd+]** repeatedly past the top - the spoken rate stops climbing at
  **400** (clamp), no error.
- [ ] Press **Ctrl+Cmd+[** repeatedly past the bottom - the spoken rate stops at **100**.
- [ ] Press **Ctrl+Cmd+V** three times - hear "Verbosity medium." → "Verbosity quiet." →
  "Verbosity everything." (wraps). The "quiet" announcement is audible even though the new
  level is quiet.

---

## Native numeric selection - live pickers (screen off)

Trigger each picker from a real `claude` session, screen off.

- [ ] **AskUserQuestion (single-select).** Ask Claude something that yields an
  AskUserQuestion. Expect Sonari to read the question, then "Option 1: …", "Option 2: …",
  then the cue ("Press the option's number to choose, or Escape to cancel."). Press a digit
  (e.g. **2**) → that option is selected **immediately** (no Enter needed). (screen off)
- [ ] **AskUserQuestion - Esc denies.** Trigger another AskUserQuestion and press **Esc** →
  it cancels/dismisses without selecting. (screen off)
- [ ] **Permission prompt.** Trigger a tool-permission prompt (e.g. a command that needs
  approval). Expect Sonari reads the action + the cue (at `everything`). Confirm
  **1 = first/proceed** and **Esc = deny**. Record the exact option order spoken. (screen off)
- [ ] **ExitPlanMode plan.** Have Claude present a plan (plan mode). Expect Sonari reads
  the plan text + cue; **1 accepts**, **Esc keeps planning**. (screen off)
- [ ] **Once-per-session warning.** On the FIRST picker of a fresh session at `everything`,
  the cue ends with "Selecting is immediate." On the SECOND picker the cue is read again but
  WITHOUT the "Selecting is immediate." tail. (screen off)

---

## O-1 - multiSelect keys (digit vs Space + Enter)

- [ ] Trigger a **multiSelect** AskUserQuestion. Expect Sonari reads the note: "Select
  multiple: press each number, or Space on the highlighted item, then Enter to confirm."
- [ ] **Verify digit-toggle:** press **2**, then **4** → both toggle on/off (record whether
  a digit toggles vs immediately submits). (screen off)
- [ ] **Verify Space-on-highlighted:** use arrows to highlight an item, press **Space** →
  it toggles. Then press **Enter** → the selection is confirmed/submitted. (screen off)
- [ ] **Record the verified working keys** here. If the narrated note does not match real
  behavior, fix `_choice_notes` in `src/sonari/daemon.py` and re-run the daemon tests.

**O-1 result (fill in):** _____________________________________________

---

## O-2 - multi-question AskUserQuestion (Tab)

- [ ] Trigger a **multi-question** AskUserQuestion (more than one sub-question in one
  picker). Expect Sonari reads them in order.
- [ ] **Digit scope:** press a digit and confirm it selects within the **current**
  sub-question (not the whole picker). (screen off)
- [ ] **Tab behavior:** press **Tab** and confirm whether it **advances to the next
  sub-question** or **submits** the whole picker. Record which. (screen off)
- [ ] If Tab advances, consider adding a "Tab moves to the next question." note to
  `_choice_notes`; record the decision here.

**O-2 result (fill in):** _____________________________________________

---

## >9 options - arrow fallback

- [ ] Trigger a picker with **10 or more** options. Expect Sonari reads "More than nine
  options; use arrow keys for ten and up." in **any** verbosity.
- [ ] Confirm digits **1–9** still select the first nine, and the **arrow keys** reach
  options **10 and up** (then Enter / native confirm selects the highlighted one). (screen off)

---

## "Other" / free-text option

- [ ] Trigger an AskUserQuestion whose options include an **"Other"/free-text** entry.
  Confirm Sonari reads it among the numbered options (e.g. its number + "Other").
- [ ] Select the "Other" option by its number; confirm focus drops into the text field so
  you can type a custom answer, then Enter submits. (screen off)
- [ ] **Known Claude Code quirk:** a digit typed *while in* the free-text field still acts
  as option-select (so a custom answer that *starts with a digit* isn't possible via the
  picker). Note whether this still reproduces on this version - if so, decide whether Sonari
  should warn; if Anthropic has fixed it, drop the caveat from the docs.

---

## Re-read (Ctrl+Cmd+O)

- [ ] **Re-read replays exactly.** With a picker on screen, press **Ctrl+Cmd+O** → the
  EXACT same text (question + numbered options + cue + any multiSelect/>9 notes) is
  re-spoken. (screen off)
- [ ] **Re-read clears after flush.** Make a selection / submit / Esc to dismiss the picker,
  then trigger a NEW unrelated prompt (causes a flush), then press **Ctrl+Cmd+O** → Sonari
  says "No options to repeat." (screen off)

---

## O-3 - does the permission_prompt payload carry the options?

- [ ] **Capture a golden payload.** While a permission prompt is live, capture the raw
  `Notification` (permission_prompt) hook payload Claude Code sends (use the capture
  launcher / inspect the hook input). Inspect whether the payload includes the option list
  (the choices the user can pick), or only the action string.
- [ ] **Record yes/no.** If the payload **does** carry options, file a follow-up to enrich
  `_permission_text` in `src/sonari/daemon.py` to read the numbered options (today it reads
  only the action + cue). If it does **not**, note that and close O-3.

**O-3 result (fill in):** _____________________________________________

---

## LaunchAgent vs shell-launched parity

- [ ] **Agent-launched works.** Freshly after `sonari install` (hotkeyd started by the
  `com.sonari.hotkeyd` LaunchAgent), confirm one combo (e.g. Ctrl+Cmd+V) fires. (screen off)
- [ ] **Shell-launched works identically.** Stop the agent's process
  (`launchctl unload ~/Library/LaunchAgents/com.sonari.hotkeyd.plist`), then run the binary
  directly in a shell: `~/.sonari/sonari-hotkeyd`. Repeat the SAME combo → identical
  behavior (same combos register, same reactions, no extra beep/leak). (screen off)
- [ ] **Restore the agent.** Stop the shell instance (Ctrl+C) and reload the agent
  (`launchctl load ~/Library/LaunchAgents/com.sonari.hotkeyd.plist`) so the normal setup is
  back. Confirm the combo still fires.

---

## Sign-off

- [ ] All 9 hotkeys fire with no character leak and no beep in Terminal.app, iTerm2, and
  VS Code.
- [ ] Native numeric selection works screen-off on AskUserQuestion, permission, and plan.
- [ ] O-1 (multiSelect keys), O-2 (multi-question Tab), O-3 (permission payload), and
  O-4 (no leak/beep + LaunchAgent parity) are recorded above.
- [ ] Any wording fixes (e.g. multiSelect/Tab notes) were applied to `daemon.py` and the
  daemon tests re-run green.
