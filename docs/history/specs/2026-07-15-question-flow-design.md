# Question flow: clean lead-ins, bounded hold, answer-triggered catch-up

**Date:** 2026-07-15
**Status:** approved by user ("looks good")

## Problems (user-reported, mechanisms confirmed)

1. Mid-response questions read their lead-in RAW ("let me check out this
   repo...") - `_SUMMARY_MIN_CHARS` (daemon.py) speaks lead-ins under 280
   chars as-is, so the digest's noise-cutting never sees exactly the text
   that is mostly process narration.
2. Nothing happens when the user ANSWERS a question: no hook event exists
   (only PreToolUse), so the daemon keeps reading the pre-question backlog,
   and a lead-in digest that lands after the answer still speaks. The manual
   flush (Ctrl+Alt+Down, FLUSH_SESSION) skips the channel backlog but leaves
   `_pending_decision`, `_held_decision`, and in-flight digests alive.
3. Questions are held for context-first ordering until the lead-in digest
   lands - a live 10-40s model call of silence before a blocking question.

## Design

### A. Lead-ins before questions are always digested, never raw
- `_maybe_summarize(session, leadin_for_decision=False)`: when True and the
  gathered text is under `_SUMMARY_MIN_CHARS`, dispatch the ASYNC digest
  (same as the long path) instead of the raw replay; return True so the
  caller holds the question. `_settle_fire`'s pending-decision branch passes
  True; the plain turn-end path keeps today's behavior.
- The digest worker gains a `leadin` flag: a falsy digest (SKIP / empty /
  failed) for a lead-in is DROPPED silently (the question still speaks via
  the held-release path) instead of falling back to the raw text. Turn-end
  digests keep the always-read raw fallback.

### B. Catch-up on answer + stronger flush
- New protocol message `CHOICE_ANSWERED` ("choice_answered"); hooks_entry
  maps PostToolUse(tool_name == AskUserQuestion) to it (all other PostToolUse
  events map to []); hooks/hooks.json registers PostToolUse with matcher
  AskUserQuestion. Takes effect in NEW Claude Code sessions (hook config is
  read at session start).
- New daemon helper `_user_caught_up(session)`: skip the session channel's
  backlog non-destructively (cursor advance, `_pending_heard` markers popped,
  same as FLUSH_SESSION), cut the current utterance if it belongs to the
  session, drop `_pending_decision` + cancel the settle window, drop
  `_held_decision`, discard `_await_choice`, and advance `_summary_gen` +
  zero `_inflight_digests` so any in-flight digest lands dead ("user
  answered"). `_voiced_upto` stays (post-answer digests cover only new
  prose). Returns whether anything was dropped/cut.
- `CHOICE_ANSWERED` handler calls it (no earcon: answering is its own
  feedback). FLUSH_SESSION additionally calls the summary-mode extras
  (pending/held/settle/gen) so the manual Down press has identical semantics.
- nav_next (Ctrl+Alt+Right) in summary mode stays a silent no-op; the
  catch-up gesture is Ctrl+Alt+Down (flush), which the user already has.

### C. Question hold capped at 5 seconds
- `_DECISION_HOLD_MAX_S = 5.0`. When `_enqueue_or_hold_decision` holds an
  item, it arms a daemon Timer via a `_schedule_hold_release(session, owner,
  item)` seam (tests call `_release_held_decision` directly). On fire, under
  the lock: if `_held_decision[session]` is still exactly (owner, item), pop
  and append the question to the session channel + wake. The digest worker
  landing later finds no held entry and just plays the digest after the
  question - bounded inversion, and if the user already answered, the gen
  bump from B drops it entirely.

## Interactions
Question speaks within settle (0.6s) + at most 5s; its lead-in digest follows
only if it survived SKIP; answering (or Ctrl+Alt+Down) silences everything
stale and future stale digests. Non-summary mode is untouched.

## Testing
protocol round-trip; hooks_entry PostToolUse mapping (AskUserQuestion vs
other tools); daemon: caught-up drops backlog/current/pending/held/in-flight,
flush_session parity in summary mode, hold release fires once and is
idempotent vs the worker, short lead-in with pending decision dispatches
async + no raw replay, lead-in SKIP drops silently while turn-end SKIP keeps
raw fallback; hooks.json contains the PostToolUse block.
