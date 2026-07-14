# Summary Mode: Question Lead-In Settle - Design

**Issue:** Maxaubert/Sonara#16
**Status:** Approved (extends the #14 settle window; same mechanism)
**Date:** 2026-07-13

## Goal

In summary mode, a question's lead-in context is always read before the
question, even when the question signal reaches the daemon before its text.

## Problem (confirmed via sequence logging)

Same turn-boundary race as #14, on the decision path. The `AskUserQuestion` ->
CHOICE message (PreToolUse hook) reaches the daemon before the lead-in text
(MessageDisplay hook). `_maybe_summarize` runs at CHOICE time on an empty
lead-in, so the question is enqueued alone and the real context (arriving
0.02-0.09s later) is recorded but never voiced. A blocking question never
reaches `turn_done`, so nothing re-triggers the digest and the context is
stranded.

Log: `MSG choice sess=594e93` at t=931899.94, `MSG prose sess=594e93` at
t=931900.03; question spoken at t=931902.14 with no context digest.

## Design: defer the question's content through the settle window

Reuse the #14 per-session settle window. On a CHOICE in summary mode:

1. Immediately (order/mode-independent): build the question item, set
   `_options`, set `_await_choice` (so the paired permission notification is
   still suppressed), and record the choice to history. The attention chime is a
   separate EARCON that already fired instantly.
2. Store the question item in `_pending_decision[session]` and **arm the settle
   window** instead of gathering the lead-in now.
3. Each PROSE delta for the session resets the window (existing #14 behavior via
   `_settle_pending`), so late lead-in text is absorbed.
4. When the window fires: gather the lead-in (`_maybe_summarize`) and
   `_enqueue_or_hold_decision(session, item, digesting)` -- context is digested,
   the question is held until the digest lands, so context is heard first.

`_settle_fire` disambiguates: if `_pending_decision` has the session it runs the
decision flow; otherwise it runs the plain turn-end digest (#14). A question
blocks the turn (no `turn_done`), so the two never contend for one turn.

Non-summary mode keeps the original immediate CHOICE path (prose is spoken live;
no digest to order).

`FLUSH` cancels a pending question settle (clears `_pending_decision` alongside
the existing `_held_decision`/`_await_choice` resets). `SESSION_END` clears it.

## Test scenarios (acceptance)

1. **Lead-in after choice is not stranded.** Send CHOICE, then the lead-in prose,
   then fire the settle: the context digest is enqueued/held and the question
   follows it (context before question), and the digest text contains the
   lead-in.
2. **Deferral.** CHOICE alone does not enqueue the question or gather the lead-in
   until the settle fires.
3. **Existing question behavior preserved.** Held-until-digest, short-lead-in
   synchronous, no-lead-in, raw-fallback, held-question-played-on-failure, and
   the multi-session held-question-via-session-channel tests all still pass once
   the settle fires (the `_choice` test helper fires the window deterministically).
4. **FLUSH cancels.** A new prompt during a question's settle window drops the
   pending question and its late fire is a no-op.

## Out of scope

- PLAN (ExitPlanMode) and PERMISSION lead-in races (same shape; separate fix).
- The `final`-flag early release (a later responsiveness optimization).
