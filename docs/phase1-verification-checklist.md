# Echo Phase 1 — Manual Eyes-Free Verification Checklist

This is the human exit-criteria check for Phase 1. It validates real audio against
the spec's success criterion: **a full session with the screen off.**

Perform on the target Mac after install.

---

## Setup

1. `pip install -e .` and `claude plugin add ~/projects/claude-tts` are done.
2. `echo doctor` reports all checks pass (enhanced voice present, daemon up, socket
   reachable, all seven hooks registered).
3. `echo verbosity everything` and set a comfortable `echo rate` (e.g. 200).
4. **Turn the screen off / look away. Do the rest by ear only.**

---

## Checklist

Start a Claude Code session and confirm each item by sound alone:

- [ ] **Session start.** Starting the session plays the **ready** earcon (Glass).
- [ ] **Prose in order.** Ask Claude something that produces multi-sentence prose; it is
      spoken sentence-by-sentence, in order, in your enhanced voice, with no stutter and no
      double-speaking.
- [ ] **Code summary.** Ask for a code block; you hear "*N-line `<lang>` code block*", not
      the code character-by-character.
- [ ] **Choice question.** Trigger an `AskUserQuestion`. A **choice** earcon (Ping) fires
      *immediately*, but the question and its numbered options are spoken only **after** the
      preceding prose finishes. The numbers match the on-screen picker.
- [ ] **Plan.** Trigger `ExitPlanMode`. A **plan** earcon (Submarine) fires immediately; the
      plan text is spoken in order after any preceding prose.
- [ ] **Permission.** Trigger a permission prompt (e.g. a `Bash` command). A **permission**
      earcon (Funk) fires immediately; the action ("Run: …") is spoken in its natural place.
- [ ] **No barge-in on detail.** While prose is still speaking, make a decision appear and
      confirm the *spoken detail* of the decision does **not** cut off the prose — only the
      earcon barges in.
- [ ] **Turn done.** When Claude finishes a turn, a **turn_done** earcon (Tink) plays.
- [ ] **Flush on new prompt.** Submit a new prompt mid-speech; the backlog is flushed and the
      voice resumes on the new turn.
- [ ] **Stop.** Run `/echo:stop`; speech stops immediately and the queue is cleared.
- [ ] **Verbosity.** `/echo:verbosity quiet` then run a tool — you hear no tool
      announcement; switch back to `everything` and tool announcements return.
- [ ] **Per-session.** Open a second Claude Code session. With the first in the foreground,
      drive the second toward a decision: you hear its decision **earcon** but **not** its
      prose. Bring the second forward (submit a prompt in it) and confirm it now speaks.
- [ ] **No system-wide kill.** Start unrelated `say "hello from another app"` in a Terminal,
      then trigger Echo speech; the unrelated `say` is **not** killed (Echo only cancels its
      own child).

---

## Pass Criteria

**Pass = every box checked with the screen off.**

If any box fails, file the failure against the owning component:

| Symptom | Component to investigate |
|---|---|
| Earcon/ordering issues | daemon + queue |
| Missing options/plan | `hooks_entry` |
| Wrong/robotic voice | `speaker` + voice setup |
| Wrong session spoken | `sessions` |

Re-run the relevant automated test before re-checking the manual item.

When every box is checked, Phase 1 is complete.
