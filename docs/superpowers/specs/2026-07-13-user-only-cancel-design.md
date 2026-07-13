# Summary Mode: Only the User Cancels — Design

**Issue:** Maxaubert/Sonara#13
**Status:** Draft for review
**Date:** 2026-07-13

## Goal

In summary mode, guarantee that the system never silently drops a completed
message. A session's most recent finished message is always read; the only
thing that removes a reading is a real user action on that session.

## The two hard rules (invariants)

**Rule 1 — last message per session is sacred.** Each session's most recent
*completed* message is always read in full. It may queue behind another
session that is currently speaking, but it must never be skipped, dropped, or
silently digested away by the system's own timing.

**Rule 2 — answer-response is sacred.** When a session asks a question, the
user answers, and the AI's next response builds on that answer, that response
must be read in full — never half, never lost to timing. This is the
non-negotiable case (it is the one that failed in testing).

Reconciliation that makes both consistent with cancellation: **"last message
always read" means "never lost to the system's own timing or bugs — only ever
cancelled by the user."** If the user deliberately moves on with a new prompt,
that is the user choosing to skip, which is fine.

## Cancellation model: only the user cancels

Two — and only two — things cancel a session's reading:

1. **New prompt to a session** → cut off that session's current and pending
   reading. The user has the gist and has moved on.
2. **Answering that session's question** → flush the question/context audio
   still playing (the user has answered; no need to keep hearing the question),
   then arm the AI's reply-to-the-answer to be read in full.

**The system never cancels.** Once the AI finishes a message it is locked in to
be read. Only a later user action on that session removes it. None of these may
ever drop it:
- a newer turn of the same session merely *starting* (before it finishes),
- a background digest or a slow (~15 s) summary landing,
- another session speaking,
- any internal generation/counter bookkeeping.

Rule 2 then falls out of the general rule for free: after the user answers, the
AI's reply is simply the session's next completed message, so it is locked in
and only a *new* user prompt can remove it. No special-casing of the
answer-response is required.

## Root cause in current code

`src/sonara/daemon.py`:

- **Turn-end** fires a `turn_done` earcon (`daemon.py:464`) → `_maybe_summarize`
  (`daemon.py:470`) → dispatches an async digest, **bumping the per-session
  generation counter** `_summary_gen` (`daemon.py:1042-1043`).
- **Digest completion** (`_summary_worker`) drops the result if the counter
  moved: `if self._summary_gen.get(session) != gen: return` (`daemon.py:1095`).
- The counter bumps in two places:
  - `FLUSH` — a new user prompt (`daemon.py:483`). Dropping here is **correct**:
    it is a real user action.
  - a later **turn-end dispatch** (`daemon.py:1042`). Dropping here is the
    **bug**: a new turn merely *starting* is not a user action, yet it discards
    the previous turn's already-finished message.

So a completed message (including an answer-response) is discarded by the
system's own bookkeeping. That breaks both rules.

## The fix

**Cancellation is driven only by user actions, never by an internal counter a
new turn-end bumps.**

### Core change (satisfies both hard rules)

- Stop the turn-end digest dispatch from invalidating a previously-dispatched
  digest. A digest that has finished is enqueued to play; it is superseded
  **only** by a user action on that session.
- Keep `FLUSH` (new prompt) as a cancel: it drops that session's current/pending
  audio and any in-flight digest, exactly as today (`daemon.py:473-492`).
- Completed digests **queue** and play in order; a pending (not-yet-played)
  digest for a session is removed only when the user acts on that session.

Concretely, the supersede decision at `daemon.py:1095` must key off "has the
user acted on this session since dispatch?" rather than "has any newer turn-end
occurred?". The implementation approach (separate the user-action counter from
turn-end dispatch, so only `FLUSH`/answer advance it) is fixed in the plan.

### Secondary enhancement (snappiness — optional, may be a separate task)

- **Flush on answer.** When the user answers a session's question, cut off that
  session's still-playing question/context audio immediately, then let the
  reply-to-the-answer read fresh. This makes the reply appear to arrive sooner
  (today the read-aloud lags the actual content). The user flagged this as "not
  a hard requirement," so it is separable from the core change and can ship
  after it.

## Open question for review

**When one session produces several finished messages in a row with no user
action between them** (e.g. autonomous multi-step work, or a background session
generating paragraph after paragraph on separate prompts): read **all** of them
queued, or only the **most recent**?

- Recommendation: **queue all**. It matches the model ("the system never drops")
  and the earlier paragraph-after-paragraph test, where each paragraph should be
  read. Each is only cancelled if the user prompts that session again before it
  is read.

## Test scenarios (acceptance)

1. **Answer-response never lost (Rule 2).** Session asks a question → user
   answers → AI replies. The reply is read in full, even if its digest takes
   ~15 s. Only a *new* user prompt to that session cancels it.
2. **Last message never lost (Rule 1).** Session A finishes a message while
   Session B is speaking. A's message is not dropped; it plays (queued) after B,
   with the "Session changed" handoff announced before it.
3. **User-cancel on new prompt.** Reading Session A's response; user prompts A
   again → current reading cuts off; A's new response reads when done.
4. **Cross-session does not cancel.** Reading Session A; user prompts Session B →
   A is not cancelled (queues); B reads after A (or A after B per current
   foreground/handoff policy) — neither is dropped.
5. **Paragraph-after-paragraph.** Prompt A, prompt B, prompt B again, prompt A
   again (each its own prompt). Every completed turn is read except ones the
   user cancelled by re-prompting the *same* session before it read.
6. **Flush on answer (if built).** While the question is still being read, the
   user answers → question audio cuts off → reply reads fresh.

## Out of scope

- Pre-rendered instant control cues (Muted/Paused/Session changed) — tracked
  separately.
- Faster/streamed digests to shrink the ~15 s window — a later optimization;
  this design makes correctness independent of digest latency.
