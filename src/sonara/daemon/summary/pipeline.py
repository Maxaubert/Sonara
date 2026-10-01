"""Summary mode: turn-end and lead-in digests (#13, #14, #16, #83, #88).

A turn's prose is recorded to history but not spoken live; once the turn
settles it is recapped by a throwaway summarizer call (summarizer.py) or,
when short, spoken raw. A blocking question (choice, plan, permission) waits
for its lead-in context: it is held behind the lead-in digest and released
when that digest lands, fails or hits the hold cap.

Everything here runs under the daemon lock unless it says otherwise: the
settle fire, the hold release, the watchdog and the summarizer worker run on
their own threads and take the lock themselves.

The pipeline keeps the daemon's config, history, router, sessions, digest
store, reorder buffer, shared state and heard-marker map by reference, so the
daemon must not rebind those attributes after construction."""
from __future__ import annotations

import sys
import threading
import time

from sonara import config_schema

# Summary mode: a turn whose prose is already shorter than this is spoken
# as-is instead of being digested (a digest of a short message adds nothing,
# costs a model call, and risks spoken meta-text on borderline input).
# EXCEPTION (#83): a lead-in before a pending QUESTION is digested even when
# short - short mid-turn lead-ins are precisely the "let me check the repo"
# process narration the digest exists to cut.
_SUMMARY_MIN_CHARS = 280

# Seconds added to summary_timeout to get the hold cap below. Covers the gap
# between the summarizer subprocess timing out and the worker's finally
# actually running (process teardown, post-processing).
_DECISION_HOLD_GRACE_S = 5.0


def _decision_hold_max_s(config) -> float:
    """Max seconds a blocking question is HELD behind its in-flight lead-in
    digest (#83). This is the WEDGE guard for a summarizer that never returns
    AT ALL, not a bound on slow ones: the digest worker's finally releases the
    question the instant the digest lands OR fails OR SKIPs, and the question's
    attention earcon fires immediately regardless.

    So the cap must outlive any digest the summarizer can still come back from,
    which is exactly summary_timeout. A hardcoded value cannot: #103 raised it
    5s -> 30s against codex latency (median 8.7s / p90 17.7s), and switching the
    engine to claude+haiku tripled that (median 24.6s, 25% of digests past 30s)
    with summary_timeout untouched at 60. The guard then fired on healthy work
    and made the "bounded inversion" (question before its context) common again
    (#121). Deriving it means any engine or timeout change carries the cap along.

    Past the cap the question speaks and the digest follows."""
    return _summary_timeout_s(config) + _DECISION_HOLD_GRACE_S


def _summary_timeout_s(config) -> float:
    """summary_timeout from the live config as a finite, non-negative float."""
    try:
        timeout = float(config_schema.get(config, "summary_timeout"))
    except (AttributeError, TypeError, ValueError):
        timeout = 60.0
    if timeout != timeout or timeout in (float("inf"), float("-inf")):
        timeout = 60.0                      # NaN / inf from a hand-edited config
    return max(timeout, 0.0)


def _digest_watchdog_s(config) -> float:
    """Seconds after dispatch at which a turn-end digest slot that has still
    not landed is landed by the watchdog (#138, audit M1). The summarizer
    enforces summary_timeout itself, so a worker still out at twice that is
    hung, and its slot would otherwise park every later digest forever."""
    return 2.0 * _summary_timeout_s(config)


# How long an ended session keeps its cancel and settle generations (#161).
# They must outlive any worker, timer or parked digest dispatched before the
# end, so a late one still finds the generation moved; after that they are
# pruned, or a long-running daemon keeps an entry for every session it saw.
_ENDED_KEEP_MIN_S = 3600.0


def summary_log(reason) -> None:
    """One summary-pipeline line on stderr, which the supervisor redirects to
    speechd.log, so a silent recap failure is diagnosable."""
    print("[summary] {0}".format(reason), file=sys.stderr, flush=True)


class SummaryPipeline:
    """Owns the summary-mode state. Given the daemon's shared state
    explicitly: *digests* is the reorder buffer (summary/reorder.py),
    *shared* the core.SharedState (Up's re-read text), *pending_heard* the
    item id -> history entry map; *enqueue*, *replay* and *earcon* are the
    daemon's own enqueue, replay and earcon paths."""

    def __init__(self, config, lock, wake, router, sessions, history,
                 digest_store, digests, shared, pending_heard, enqueue,
                 replay, earcon) -> None:
        self._config = config
        self._lock = lock
        self._wake = wake
        self._router = router
        self._sessions = sessions
        self._history = history
        self._digest_store = digest_store
        self._digests = digests
        self._shared = shared
        self._pending_heard = pending_heard
        self._enqueue = enqueue
        self._replay = replay
        self._earcon = earcon
        self._log = summary_log
        # Per-session CANCEL epoch. Only a user action (a new prompt -> FLUSH)
        # advances it; a finished digest is dropped iff the epoch moved since
        # it was dispatched. A turn merely ending does NOT advance it, so the
        # system never drops a finished message -- only the user cancels (#13).
        self.cancel_gen: dict = {}
        # Per-session turn-end SETTLE window (#14). turn_done can reach the
        # daemon before a turn's final prose (separate hook processes race
        # under multi-session load), so digesting immediately summarizes an
        # incomplete/empty turn. Arm a short window on turn_done, reset it on
        # each new prose delta, and digest only once the session is quiet.
        self.settle_timers: dict = {}     # session -> threading.Timer
        self.settle_gen: dict = {}        # session -> int (stale-fire guard)
        self.settle_pending: set = set()  # sessions with a window armed
        # session -> decision items awaiting their lead-in (#16), in arrival
        # order: a second decision inside the settle window used to overwrite
        # the first, which was then never spoken (#138, audit F7).
        self.pending_decision: dict = {}
        # session -> (owner token, [decision items]) held until the lead-in
        # digest lands (context-first ordering).
        self.held_decision: dict = {}
        # Digest dispatch bookkeeping (#21): each dispatch gets a token, and a
        # held decision is OWNED by the dispatch it waits behind -- only that
        # worker may pop and append it. Without ownership, whichever same-gen
        # worker landed first stole the question and played it before its own
        # lead-in context. inflight lets a decision with no NEW prose of its
        # own still hold behind an earlier digest that is mid-flight.
        self.dispatch_token = 0            # monotonically increasing dispatch id
        self.last_dispatch_token: dict = {}   # session -> newest dispatch token
        self.inflight: dict = {}              # session -> workers in flight
        # session -> last HistoryEntry voiced this turn (a blocking question and
        # turn-end must not double-voice; identity survives history-cap
        # eviction, audit #21).
        self.voiced_upto: dict = {}
        self.summarize_fn = None      # test seam; None -> sonara.summarizer.summarize
        # session -> clock time it ended; its generations are pruned once it
        # has been over for ended_keep_s() (#161).
        self.ended_at: dict = {}
        self._clock = time.monotonic  # test seam

    # -- entry points from handle_message (caller holds the lock) ----------

    def on_prose(self, session: str) -> None:
        """Late prose after turn_done: reset the settle window so the turn-end
        digest waits for the full turn to land (#14). Only when armed."""
        if session in self.settle_pending:
            self.arm_settle(session)

    def on_turn_done(self, session: str) -> None:
        """Do NOT digest yet: the turn's final prose can arrive after this
        signal (separate hook processes race). Arm a settle window and digest
        once the session is quiet (#14). Non-summary mode has no digest, so
        nothing to defer there."""
        if self._config.get("summary_mode"):
            self.arm_settle(session)

    def on_decision(self, session: str, item) -> None:
        """A blocking question (choice, plan, permission). Summary mode defers
        the lead-in digest + question through the settle window so late
        lead-in prose is included and heard before the question (#16, audit
        #21). Non-summary speaks prose live, so the question is enqueued (or
        held behind an in-flight digest) immediately."""
        if self._config.get("summary_mode"):
            self.pending_decision.setdefault(session, []).append(item)
            self.arm_settle(session)
        else:
            self.enqueue_or_hold_decision(session, item, False)

    def cancel(self, session: str) -> None:
        """A new prompt (FLUSH) or the session ending: the user cancelled this
        session's summary work. Advance the cancel epoch so any digest
        dispatched before now is dropped when it lands, rather than spoken
        into the new turn (#13). Popping it instead reset never-FLUSHed
        sessions to a PASSING guard (get()==0 == the dispatched gen 0), letting
        a dead session's digest resurrect its history and channel (audit #21).

        The settle window is cancelled, not popped: _cancel_settle BUMPS the
        settle generation, and popping restarted the next arm at gen 1, so a
        fire from before the teardown, blocked on the lock, passed the stale
        guard (#138, audit L-settle-pop). Held and settle-deferred questions
        are dropped. Cancelled digests no longer count as in flight: a stale
        count held the NEW turn's question hostage behind a dead worker
        (silent up to summary_timeout, probe-confirmed; deep audit #25). The
        worker's finally skips its decrement when its gen is stale."""
        self.cancel_gen[session] = self.cancel_gen.get(session, 0) + 1
        self.cancel_settle(session)
        self.drop_decisions(self.pending_decision.pop(session, None))
        self.drop_held_decisions(session)
        self.voiced_upto.pop(session, None)       # new turn: nothing voiced yet
        self.inflight.pop(session, None)
        self.last_dispatch_token.pop(session, None)

    def end_session(self, session: str) -> None:
        """SESSION_END or FORGET_SESSION: cancel the session's summary work
        like a FLUSH, remember when it ended, and prune the generations of
        sessions that ended long ago (#161). The generations are not popped
        here: a worker or timer dispatched before the end may still land."""
        self.cancel(session)
        now = self._clock()
        self.ended_at[session] = now
        self.prune_ended(now)

    def ended_keep_s(self) -> float:
        """How long an ended session's generations are kept: well past the
        digest watchdog (twice summary_timeout), so no stale work survives
        its session's pruning."""
        return max(_ENDED_KEEP_MIN_S, 4.0 * _summary_timeout_s(self._config))

    def prune_ended(self, now: float) -> None:
        """Drop cancel_gen and settle_gen for sessions ended over
        ended_keep_s() ago with no summary work left."""
        keep = self.ended_keep_s()
        busy = self.busy_sessions() | set(self.settle_timers)
        for session, ended in list(self.ended_at.items()):
            if now - ended < keep or session in busy:
                continue
            self.cancel_gen.pop(session, None)
            self.settle_gen.pop(session, None)
            del self.ended_at[session]

    def caught_up(self, session: str) -> bool:
        """The summary-mode half of a caught-up user (#83, #107): drop a
        settle-deferred or digest-held question, kill the settle window and
        any in-flight lead-in digest (its gen guard drops the result "user
        answered", exactly like a new prompt's FLUSH does, #13), and advance
        the voiced marker past everything already said, so the post-answer
        turn-end digest never re-includes the pre-question lead-in (it was
        skipped, not merely delayed). Returns True when anything was
        dropped."""
        pending = self.pending_decision.pop(session, None)
        self.drop_decisions(pending)
        dropped = bool(pending)
        # A live settle window means a digest was ABOUT to dispatch: killing
        # it is a user-visible silencing, so it counts as dropped (#107).
        dropped = (session in self.settle_pending) or dropped
        self.cancel_settle(session)
        dropped = self.drop_held_decisions(session) or dropped
        if self.inflight.get(session):
            self.cancel_gen[session] = self.cancel_gen.get(session, 0) + 1
            self.inflight.pop(session, None)
            self.last_dispatch_token.pop(session, None)
            dropped = True
        entries = [e for mid in self._history.message_ids(session)
                   for e in self._history.entries_for_message(session, mid)
                   if e.kind == "prose"]
        if entries:
            self.voiced_upto[session] = entries[-1]
        return dropped

    def busy_sessions(self) -> set:
        """Sessions with summary work cooking: a digest in flight, a question
        deferred or held, or a settle window armed (flush-to-end, #107)."""
        return (set(self.inflight) | set(self.pending_decision)
                | set(self.held_decision) | set(self.settle_pending))

    def drop_decisions(self, items) -> None:
        """Forget the heard-markers of decision items that will never reach a
        channel (pending or held ones dropped by FLUSH, teardown or a
        catch-up), so _pending_heard can't leak (#138, audit F7)."""
        for it in items or ():
            self._pending_heard.pop(it.id, None)

    def drop_held_decisions(self, session: str) -> bool:
        """Drop a session's held decisions with their bookkeeping. Returns True
        when anything was held."""
        held = self.held_decision.pop(session, None)
        if held is None:
            return False
        self.drop_decisions(held[1])
        return True

    # -- dispatch -------------------------------------------------------------

    def maybe_summarize(self, session: str,
                        leadin_for_decision: bool = False) -> bool:
        """Summary mode: recap the session's prose not yet voiced this turn via a
        throwaway claude -p call (see summarizer.py), or speak it raw when short.
        Runs under the daemon lock, so it only gathers text and spawns the worker
        thread; the subprocess itself runs OFF-lock in worker().

        Called at turn end AND when a blocking decision arrives: a question never
        reaches turn_done, so its lead-in prose would otherwise be silently dropped.
        Only prose recorded SINCE the last call this turn is voiced (tracked by
        voiced_upto), so the two triggers never double-voice the same text.

        *leadin_for_decision* (#83): the gathered text precedes a pending
        QUESTION. Short lead-ins are then DIGESTED instead of replayed raw
        (mid-turn narration like "let me check the repo" is exactly the noise
        the digest cuts), and a SKIP/failed lead-in digest drops silently
        instead of falling back to the raw text.

        Returns True iff an ASYNC digest was dispatched (long lead-in) -- the
        decision handlers use this to HOLD the question until the digest lands, so
        the context is heard before the question rather than ~6s after it."""
        if not self._config.get("summary_mode"):
            return False
        leadin = bool(leadin_for_decision)
        entries = []
        for mid in self._history.message_ids(session):
            for e in self._history.entries_for_message(session, mid):
                if e.kind == "prose":
                    entries.append(e)
        # Skip everything up to and including the last entry voiced this turn,
        # located by IDENTITY. An absolute count desynced from the CAPPED history
        # deque once eviction shifted it, over-skipping unvoiced prose -- worst
        # case gathering nothing and silently dropping the turn-end digest
        # (audit #21). A marker that was itself evicted means every surviving
        # entry is unvoiced: keep them all.
        marker = self.voiced_upto.get(session)
        if marker is not None:
            for i in range(len(entries) - 1, -1, -1):
                if entries[i] is marker:
                    entries = entries[i + 1:]
                    break
        text = " ".join(e.text for e in entries).strip()
        if not text:
            return False                 # decision-only / empty / already-voiced
        self.voiced_upto[session] = entries[-1]
        if len(text) < _SUMMARY_MIN_CHARS and not leadin:
            # An already-short turn needs no digest: speak the original prose
            # instead. Digesting borderline-trivial input made the model
            # verbalize meta-text ("no content to be spoken") that was then
            # read aloud; speaking the original is faster and free.
            if self._sessions.is_foreground(session):
                # Append (not cursor-insert) so a short turn never overtakes a
                # queued question -- consistent with the long digest path (#17).
                # A turn delivery must ANNOUNCE on a real reader switch (#21).
                self._replay(session, entries, append=True,
                             suppress_announce=False)
                # Up re-reads the joined text -- parity with the background
                # short-turn and digest paths; without this, Up after a short
                # foreground turn gave a dead edge chime (deep audit #25).
                self._shared.last_digest_text[session] = text
                self._digest_store.set(session, text)
            else:
                # Background sessions are not voiced from their own channel;
                # speak the short turn via the session channel. It joins the
                # digest SEQUENCE (#88): a short turn finishing after a long
                # one must not jump ahead of the long turn's cooking digest.
                gen = self.cancel_gen.get(session, 0)

                def _deliver():
                    # Runs at RELEASE time, possibly after waiting parked
                    # behind an earlier digest (F2): the same cancel guard as
                    # the worker's apply(), so a new prompt or a session end
                    # in the meantime drops it instead of speaking stale text
                    # or resurrecting the ended session.
                    if self.cancel_gen.get(session, 0) == gen:
                        self.enqueue_background_digest(session, text)

                self._digests.land(self._digests.alloc(), _deliver)
            return False                 # spoken synchronously; no need to hold
        # Capture the session's CANCEL epoch WITHOUT advancing it. Only a user
        # action (a new prompt -> FLUSH) advances the epoch; a turn merely ending
        # must never invalidate a previously-dispatched digest. So several
        # turn-ends with no user action between them each keep their digest (they
        # queue and play) -- the system never drops a finished message (#13).
        gen = self.cancel_gen.get(session, 0)
        self.dispatch_token += 1
        token = self.dispatch_token
        self.last_dispatch_token[session] = token
        self.inflight[session] = self.inflight.get(session, 0) + 1
        # Turn-end digests get an ordering slot (#88); lead-in digests bypass
        # (latency-critical, #83) and stay seq=None.
        seq = None if leadin else self._digests.alloc()
        try:
            self.start_thread(session, gen, text, token, leadin=leadin,
                              seq=seq)
        except Exception:
            # No worker will ever land this slot (#138, audit L-settle-fire):
            # undo the in-flight count and land it now with the raw-text
            # fallback, so the turn still speaks and later digests don't park.
            n = self.inflight.get(session, 0) - 1
            if n > 0:
                self.inflight[session] = n
            else:
                self.inflight.pop(session, None)
            self._digests.land(seq, None if leadin else
                               self.digest_apply(session, gen, text, False, None))
            raise
        if seq is not None:
            # A worker that never returns would park every later digest
            # (#138, audit M1): bound the slot at twice summary_timeout.
            # The worker is already out, so a failure to arm the watchdog is
            # logged and the digest still counts as in flight.
            try:
                self.schedule_digest_watchdog(
                    seq, self.digest_apply(session, gen, text, False, None))
            except Exception:  # noqa: BLE001 - the worker still lands its slot
                import traceback
                self._log("digest watchdog arm failed for seq {0}:\n{1}".format(
                    seq, traceback.format_exc()))
        return True                      # async digest in flight -> caller holds

    # -- settle window (#14) ----------------------------------------------------

    def arm_settle(self, session: str) -> None:
        """Defer the turn-end digest until the session's prose settles. Restart the
        window on every new prose delta; fire once quiet (#14). Caller holds the
        lock (this runs from handle_message)."""
        self.ended_at.pop(session, None)   # a new turn: the session is live
        gen = self.settle_gen.get(session, 0) + 1
        self.settle_gen[session] = gen
        self.settle_pending.add(session)
        old = self.settle_timers.pop(session, None)
        if old is not None:
            old.cancel()
        self.schedule_settle(session, gen)

    def schedule_settle(self, session: str, gen: int) -> None:
        """Start the real settle timer. Test seam: tests replace this to drive
        settle_fire deterministically instead of waiting on the clock."""
        settle_s = config_schema.get(self._config, "summary_settle_ms") / 1000.0
        t = threading.Timer(settle_s, self.settle_fire, args=(session, gen))
        t.daemon = True
        self.settle_timers[session] = t
        t.start()

    def settle_fire(self, session: str, gen: int) -> None:
        """The settle window elapsed with no new prose: dispatch the turn-end
        digest now that the full turn has landed. Runs on the Timer thread, so it
        takes the lock. A stale fire (re-armed by later prose, or cancelled by
        FLUSH) is a no-op via the generation guard.

        Never raises (#138, audit L-settle-fire): an exception used to kill the
        Timer thread and lose the turn silently. maybe_summarize lands any
        digest slot it allocated before re-raising, and a decision this fire
        took over is still enqueued, so the blocking question is never lost."""
        with self._lock:
            if self.settle_gen.get(session) != gen:
                return
            self.settle_pending.discard(session)
            self.settle_timers.pop(session, None)
            items = self.pending_decision.pop(session, None) or []
            placed = 0
            try:
                if items:
                    # Questions were waiting on their lead-in: gather it now
                    # (present after the settle) and hold them after the
                    # context (#16), in arrival order. Lead-in mode (#83):
                    # short lead-ins are digested (not read raw) and a SKIP
                    # result drops instead of raw-falling-back.
                    digesting = self.maybe_summarize(session,
                                                     leadin_for_decision=True)
                    for item in items:
                        self.enqueue_or_hold_decision(session, item, digesting)
                        placed += 1
                else:
                    self.maybe_summarize(session)
            except Exception:  # noqa: BLE001 - a Timer thread must not die silently
                import traceback
                self._log("settle fire failed for {0}:\n{1}".format(
                    session, traceback.format_exc()))
                rest = items[placed:]
                # A failed hold may already have stored the item (its cap
                # timer failed to arm): take it back out so it is spoken once,
                # now, and not again when the group is released.
                held = self.held_decision.get(session)
                if held is not None:
                    kept = [it for it in held[1]
                            if not any(it is r for r in rest)]
                    if kept:
                        self.held_decision[session] = (held[0], kept)
                    else:
                        self.held_decision.pop(session, None)
                ch = self._router.channel(session)
                for item in rest:
                    ch.append(item)
                self._wake.set()

    def cancel_settle(self, session: str) -> None:
        """Drop any pending settle window: a new prompt abandons the turn. Bumps
        the generation so an already-scheduled fire becomes a no-op."""
        self.settle_pending.discard(session)
        self.settle_gen[session] = self.settle_gen.get(session, 0) + 1
        t = self.settle_timers.pop(session, None)
        if t is not None:
            t.cancel()

    # -- question hold (#16, #21, #83) ------------------------------------------

    def enqueue_or_hold_decision(self, session: str, item, digesting: bool) -> None:
        """Enqueue a decision item now, OR hold it until the lead-in digest lands
        (context-first ordering). Held items are appended by the OWNING
        worker after its digest; if the digest fails or is superseded the
        owner still enqueues the held item, so a blocking question is never lost.

        Holds not only when THIS call dispatched a digest, but also when an
        earlier digest of the same turn is still in flight -- otherwise a decision
        whose own lead-in gather found nothing new was enqueued immediately and
        played BEFORE its context (audit #21)."""
        if digesting or self.inflight.get(session, 0) > 0:
            owner = self.last_dispatch_token.get(session, 0)
            # Join any decision already held (#138, audit F7): overwriting the
            # slot dropped the earlier question. The newest dispatch owns the
            # whole group, which then speaks in arrival order.
            prev = self.held_decision.get(session)
            items = (list(prev[1]) if prev is not None else []) + [item]
            self.held_decision[session] = (owner, items)
            # Cap the hold (#83, retuned #103, derived from summary_timeout in
            # #121): the wedge guard for a hung summarizer. The normal release
            # is the digest worker's finally, which frees the question the
            # moment its context lands or fails.
            # Past the cap the question speaks and the digest follows
            # (bounded inversion; a caught-up user drops it).
            self.schedule_hold_release(session, owner, item)
        else:
            self._router.channel(session).append(item)
        self._wake.set()

    def schedule_hold_release(self, session: str, owner: int, item) -> None:
        """Arm the held-question release timer. Test seam: tests call
        release_held_decision directly instead of waiting on the clock.

        Reads the cap from the LIVE config so a summary_timeout change from the
        settings page widens the hold with it (#121)."""
        t = threading.Timer(_decision_hold_max_s(self._config),
                            self.release_held_decision,
                            args=(session, owner, item))
        t.daemon = True
        t.start()

    def release_held_decision(self, session: str, owner: int, item) -> None:
        """The hold cap elapsed: if the digest still has not landed, speak the
        question NOW (#83). Idempotent vs the digest worker: whichever runs
        first pops the hold; the other finds it gone and does nothing.

        Matches on the item, not the owner token: a later decision joining the
        hold (F7) moves ownership to the newer dispatch, and the first
        question's cap must still free the group at the EARLIEST deadline
        rather than wait out a fresh cap from the second arrival."""
        with self._lock:
            held = self.held_decision.get(session)
            if held is None or not any(it is item for it in held[1]):
                return                     # already released (digest landed / caught up)
            self.held_decision.pop(session, None)
            ch = self._router.channel(session)
            for it in held[1]:
                ch.append(it)
            self._wake.set()

    # -- hung-worker watchdog (#138) --------------------------------------------

    def schedule_digest_watchdog(self, seq: int, apply) -> None:
        """Arm the hung-worker watchdog for a dispatched turn-end digest slot
        (#138, audit M1). Test seam: tests call digest_watchdog_fire directly
        instead of waiting on the clock. Caller holds the lock."""
        t = threading.Timer(_digest_watchdog_s(self._config),
                            self.digest_watchdog_fire, args=(seq, apply))
        t.daemon = True
        self._digests.watch(seq, t)
        t.start()

    def digest_watchdog_fire(self, seq: int, apply) -> None:
        """The worker for *seq* is still out at twice summary_timeout: land its
        slot with *apply*, the raw-text fallback of a failed digest, so the turn
        is still spoken (never skip the last message) and every later digest
        parked behind it is released. A no-op once the worker has landed; the
        worker's own landing after this is ignored by DigestReorderBuffer.land."""
        with self._lock:
            self._digests.unwatch(seq)
            if self._digests.landed(seq):
                return
            self._log("digest seq {0} hung past the watchdog: "
                      "speaking the raw text".format(seq))
            self._digests.land(seq, apply)

    # -- worker -----------------------------------------------------------------

    def start_thread(self, session: str, gen: int, text: str,
                     token: int = 0, leadin: bool = False,
                     seq=None) -> None:
        threading.Thread(target=self.worker,
                         args=(session, gen, text, token, leadin, seq),
                         name="sonara-summary", daemon=True).start()

    def digest_apply(self, session: str, gen: int, text: str, leadin: bool,
                     summary):
        """The release closure for one finished digest: speak *summary*, or on
        a SKIP/empty/failed one fall back to the raw *text*. Built by the
        worker when the summarizer returns, and up front with summary=None as
        the watchdog's fallback for a worker that never returns (#138)."""
        _log = self._log

        def apply():
            # Runs at RELEASE time (#88): possibly later than completion,
            # after earlier-dispatched digests landed. State checks (gen,
            # foreground) therefore happen HERE, not at completion.
            if self.cancel_gen.get(session, 0) != gen:
                _log("digest dropped: user prompted this session since dispatch")
                return               # the user moved on -> this reading is cancelled
            out = summary
            if not out:
                if leadin:
                    # A lead-in digest that came back SKIP/empty/failed is
                    # pure process narration (#83): drop it silently. The
                    # question it contextualized still speaks via the
                    # held-release in the worker's finally - only the noise
                    # dies, never the blocking prompt.
                    _log("lead-in digest empty/SKIP: dropped")
                    return
                # SKIP / empty / failed digest. A session's LATEST message must
                # ALWAYS be read (user spec: never skip the last message --
                # digested or not). This digest is the latest (it was not
                # superseded above), so fall back to the RAW text rather than
                # dropping it. Only a genuinely empty turn stays silent.
                if not (text or "").strip():
                    self._earcon("summary_failed")
                    return
                out = text
            # A held question's context goes via the SESSION channel even when
            # the session is not foreground: it is a real handoff, so the router
            # must announce "Session changed" BEFORE the context (not at the
            # question). Route EVERY digest via its own session channel so a
            # reader switch announces the handoff ("Session changed: folder" +
            # chime) BEFORE the digest. A foreground digest does NOT switch the
            # reader (no announcement) and never carries a "Session X:" prefix:
            # the router announcement is the sole session identifier (#15).
            fg = self._sessions.is_foreground(session)
            # TTS-normalize (#27): digests bypass the assembler cleaner, so
            # markdown residue / snake_case reached the voice raw and was
            # mispronounced. Normalize BEFORE recording so Up's cache-hit
            # re-read speaks the identical string.
            from sonara.cleaner import normalize_for_speech
            out = normalize_for_speech(out)
            entry = self._history.record(session, "summary", out)
            self._enqueue(session, "summary", out, False, entry=entry)
            self._shared.last_digest_text[session] = out   # Up re-reads this verbatim
            self._digest_store.set(session, out)     # survives restarts (#118)
            ch = self._router.channel(session)
            ch.turn_done = True
            # Stamp the channel with the release index (#88): the router
            # serves waiting digest channels lowest-stamp-first, so the
            # heard order matches the turn-finish order just released.
            ch.release_order = self._digests.next_release_stamp()
            if not fg:
                # Let it be voiced + announced regardless of background policy
                # (earcon_only would otherwise mute a non-foreground session).
                self._router.authorize_replay(session)
            self._wake.set()

        return apply

    def worker(self, session: str, gen: int, text: str,
               token: int = 0, leadin: bool = False,
               seq=None) -> None:
        """Run the summarizer subprocess OFF-lock, then apply the result under the
        lock: enqueue the spoken summary, or fire the failure cue. A result whose
        generation was superseded by a newer turn end is dropped silently.
        Turn-end results release through the reorder buffer (#88, *seq*), so
        digests are heard in turn-finish order regardless of model latency."""
        from sonara import summarizer

        _log = self._log
        import time as _time
        fn = self.summarize_fn or summarizer.summarize
        t0 = _time.monotonic()
        style = config_schema.get(self._config, "summary_style")
        prompts = self._config.get("summary_prompts") or {}
        try:
            summary = fn(text,
                         model=config_schema.get(self._config, "summary_model"),
                         command=config_schema.get(self._config, "summary_command"),
                         timeout=config_schema.get(self._config, "summary_timeout"),
                         style=style,
                         instruction=prompts.get(style),
                         debug_log=_log)
        except Exception:  # noqa: BLE001 - a summary failure must never crash the daemon
            summary = None
        if summary:
            # Success trail: when a digest sounds wrong (truncated, odd), the
            # log shows exactly what the model returned vs what was spoken; the
            # duration makes latency complaints diagnosable from the log (#27).
            _log("digest ok in {0:.1f}s: {1} chars in, {2} chars out: {3!r}".format(
                _time.monotonic() - t0, len(text), len(summary), summary[:120]))
        with self._lock:
            # Questions whose lead-in this digest recaps were HELD for
            # context-first ordering; append them AFTER the digest below. Only
            # the OWNING worker (the dispatch the hold was placed behind) may
            # take them -- an earlier same-gen worker landing first must leave
            # them for their owner, or a question plays before its own context
            # (audit #21). The finally guarantees the owner plays them even on
            # a dropped/failed digest -- a blocking prompt is never lost (FLUSH
            # clears a stale one).
            held = []
            held_entry = self.held_decision.get(session)
            if held_entry is not None and held_entry[0] == token:
                self.held_decision.pop(session, None)
                held = held_entry[1]

            apply = self.digest_apply(session, gen, text, leadin, summary)
            landed = False
            try:
                self._digests.land(seq, apply)
                landed = True
            finally:
                if not landed:
                    # Landing raised: the ordering slot must still release or
                    # every later digest parks forever (#88).
                    self._digests.land(seq, None)
                # This worker is done: it no longer counts as in flight (a later
                # decision must not hold behind a digest that already landed).
                # ONLY when this worker's gen is still current: FLUSH/SESSION_END
                # already dropped a cancelled worker's count, so a stale worker
                # must not steal a POST-flush dispatch's count (deep audit #25).
                if self.cancel_gen.get(session, 0) == gen:
                    n = self.inflight.get(session, 0) - 1
                    if n > 0:
                        self.inflight[session] = n
                    else:
                        self.inflight.pop(session, None)
                if held:
                    ch = self._router.channel(session)
                    for it in held:
                        ch.append(it)            # questions after context
                    self._wake.set()

    def enqueue_background_digest(self, session: str, text: str) -> None:
        """Speak a background session's short-turn content via ITS OWN channel, so
        the router announces the handoff ("Session changed: folder" + chime) before
        it -- matching the digest path. Was on the CONTROL lane, which is silent
        (no chime), plays out of order, and survives a new prompt's FLUSH (so stale
        content lingered and replayed). Unprefixed (the announcement names it) and
        replay-authorized so the background policy does not mute it. Caller holds
        the daemon lock."""
        entry = self._history.record(session, "summary", text)
        self._enqueue(session, "summary", text, False, entry=entry)
        self._shared.last_digest_text[session] = text   # Up re-reads this verbatim
        self._digest_store.set(session, text)     # survives restarts (#118)
        ch = self._router.channel(session)
        ch.turn_done = True
        ch.release_order = self._digests.next_release_stamp()   # heard in release order (#88)
        self._router.authorize_replay(session)
        self._wake.set()
