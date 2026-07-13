# Summary Mode: Short Turn Appends (Don't Overtake a Queued Question) — Design

**Issue:** Maxaubert/Sonara#17
**Status:** Approved (targeted follow-up to #16)
**Date:** 2026-07-13

## Goal

A queued question is always read before later same-session content, so
answering before the question is voiced still yields: context, then question,
then answer-response.

## Problem (confirmed via sequence logging)

The held question is queued after the context digest. When the user answers
before the question is voiced, the AI's answer-response returns. If it is SHORT,
it takes the short-turn path (`_maybe_summarize` -> `_replay`), which inserts the
raw prose **at the channel cursor**, "ahead of any already-queued items"
(`daemon.py:991-1018`). So the answer-response cuts in ahead of the queued
question and the question reads last.

The router's `_pick` (`router.py:114`) already preempts on decisions, but that
only selects the *session*; `channel.next()` then serves whatever sits at the
cursor -- now the inserted answer-response. And the LONG-digest path appends
(`_enqueue`), so long answer-responses are already ordered correctly. Only the
short path (cursor-insert) overtakes.

Log: context summary at t=720, three kind=prose answer items at t=774-795, the
kind=choice question LAST at t=796.

## Design: short-turn content appends, like long-turn content

Give `_replay` an `append` mode. The short-turn digest path calls it with
`append=True`, so the raw prose is added at the END of the channel (after any
queued question), exactly like the long-digest path's `_enqueue`. Everything
else `_replay` does (register `_pending_heard`, mark `turn_done`, suppress the
programmatic "Session changed", authorize cross-session voicing) is unchanged.

- New content (short OR long turn) now consistently appends -> chronological
  order -> a queued question is never overtaken.
- Nav / catch-up / repeat keep the default cursor-insert (`append=False`): those
  are explicit user replays that should read next.
- The short-lead-in-before-question ordering is preserved: the lead-in appends,
  then the question appends after it (both in order).

## Test scenarios (acceptance)

1. **Short answer does not overtake a held question.** Long lead-in + CHOICE ->
   settle -> worker (context enqueued, question queued after it). Then a short
   answer-response turn arrives; after its settle the question's channel index is
   still BEFORE the answer-response's. (Fails today: the answer inserts at the
   cursor, ahead of the question.)
2. **Short lead-in still precedes its question.** A short-lead-in question still
   reads lead-in then question (append preserves order).
3. **Plain short turn unchanged.** A short turn with an empty queue speaks its
   original prose as before (append == cursor when nothing is queued).
4. **Nav/catch-up/repeat unaffected.** Those keep cursor-insert (`append=False`).

## Out of scope

- Nav/catch-up/repeat ordering (intentional cursor-insert).
- The broader multi-session digest-ordering redesign.
