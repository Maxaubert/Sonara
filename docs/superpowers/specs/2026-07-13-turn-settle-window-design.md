# Summary Mode: Turn-End Settle Window — Design

**Issue:** Maxaubert/Sonara#14
**Status:** Draft for review
**Date:** 2026-07-13

## Goal

In summary mode, ensure a turn's full text has arrived before the daemon
digests it, so multi-session content is never read partially or dropped.

## Problem (confirmed via playback-sequence logging)

Each Claude Code hook event is a separate process with its own connection to
the daemon. Streaming text arrives as `MessageDisplay` -> PROSE; the end-of-turn
marker arrives as a `Stop` -> `turn_done` EARCON. There is no ordering guarantee
between the two connections. Under multi-session load the turn's final PROSE
reaches the daemon *after* `turn_done`, so `_maybe_summarize` runs on an
incomplete or empty buffer:

- Empty buffer -> no digest dispatched; the session reads nothing.
- A few chars buffered -> under `_SUMMARY_MIN_CHARS` -> the short-turn path
  `_replay`s that raw fragment, and the real body (arriving ~0.01s later) is
  recorded to history but never re-triggers a digest; the session reads only a
  fragment.

Live log evidence: `turn_done` at t=671.40 spoke "Here is another one:"; the
paragraph body arrived t=671.41. A second session's `turn_done` at t=673.61 hit
an empty buffer (no dispatch); its whole paragraph arrived t=673.62, stranded.

This is distinct from the cancel-epoch fix (#13); it is the real multi-session
blocker and violates Rule 1 (the system must never drop content).

## Design: a per-session settle window

`turn_done` no longer means "digest now." It means "the turn is settling."

1. On `turn_done` (`daemon.py:464-470`), instead of calling `_maybe_summarize`
   immediately, **arm a settle window** for that session: a short timer that will
   call `_maybe_summarize` when it fires.
2. On every PROSE delta for a session that has a settle window pending, **reset
   the window** (restart the timer). This absorbs late-arriving final text: as
   long as text keeps landing, the window keeps deferring.
3. When the window elapses with no new prose, **fire**: acquire the daemon lock
   and call `_maybe_summarize(session)` on the now-complete turn. The short-vs-
   long decision and the digest both see the full text.
4. A new prompt (`FLUSH`) **cancels** any pending settle window for that session:
   the turn is being abandoned by the user, consistent with #13 (only the user
   cancels; here the user has moved on before the turn was even digested).

### Timing and correctness

- The settle window is short (recommended default **600 ms**, configurable via a
  new `summary_settle_ms` config key). Digests already take 6-30 s, so the added
  latency is invisible for digested turns.
- Short turns (spoken raw, no digest) gain the same ~600 ms delay before they
  speak. This is the cost of correctness: the daemon must see the whole turn
  before deciding it is "short." 600 ms is a deliberate balance; it is
  configurable if it feels sluggish.
- The timer fires on its own thread, so it must take `self._lock` before touching
  shared state (`handle_message` and its callees run under the lock; the timer
  path does not, so it wraps the call). A per-session generation guard makes a
  timer that fires just as it is re-armed or cancelled a no-op.

### Scope

- This fix covers the **`turn_done`** path (the reported bug). The decision paths
  (`CHOICE`/`PLAN`/`PERMISSION`) also call `_maybe_summarize` for lead-in prose
  and could in principle race the same way, but they carry their own held-question
  ordering and are not the reported failure. They are **out of scope** here and
  noted as a possible follow-up.
- The `final=True` flag on the last PROSE delta could let the window release
  early (dispatch as soon as the final block lands rather than waiting the full
  window). That is a later responsiveness optimization; a fixed quiet window is
  simpler and robust, so this design uses the window alone.

## Test scenarios (acceptance)

1. **Deferral.** `turn_done` alone does not dispatch a digest; only after the
   settle window fires does `_maybe_summarize` run.
2. **Late prose included.** Feed a short lead-in, `turn_done` (arms window), then
   feed the paragraph body (late prose). When the window fires, the dispatched
   digest text contains the **full** turn (lead-in + body), and it takes the
   long/digest path, not the short-fragment path.
3. **Window reset.** New prose after `turn_done` re-arms the window; a fire from
   the superseded (earlier) window is a no-op; the latest window's fire
   dispatches.
4. **FLUSH cancels.** A new prompt during the settle window cancels it; a late
   fire from the cancelled window dispatches nothing.
5. **Existing behavior preserved.** All current turn-end digest tests still pass
   once the settle fires (the test helper fires the window deterministically
   rather than waiting on the real timer).

## Out of scope

- Decision-path lead-in races (`CHOICE`/`PLAN`/`PERMISSION`).
- `final`-flag early release.
- The cancel-epoch behavior from #13 (already committed).
