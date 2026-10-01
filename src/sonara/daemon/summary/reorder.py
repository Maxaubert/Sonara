"""Digest reorder buffer (#88): turn-end digests become AUDIBLE in dispatch
(turn-finish) order, not summarizer-completion order.

Each turn-end digest gets a sequence number at dispatch. When its worker
finishes it lands its release closure under that number; the buffer runs
every consecutive ready slot from the serve pointer, so a fast digest for a
later turn waits for a slow one before it. Every method assumes the caller
holds the daemon lock (checked when SONARA_DEBUG_LOCKS=1)."""
from __future__ import annotations

from sonara.daemon.core import assert_lock_held


class DigestReorderBuffer:
    def __init__(self, lock=None, log=None) -> None:
        self._lock = lock
        self._log = log
        self.next_seq = 0                 # next sequence number to hand out
        self.serve_seq = 0                # next sequence number to release
        self.parked: dict = {}            # seq -> apply closure (None = dropped)
        self.watchdogs: dict = {}         # seq -> Timer landing a hung slot (#138)
        self._release_counter = 0         # channel stamp source (#88)

    def alloc(self) -> int:
        """Hand out the next digest sequence number (#88). Caller holds the
        lock. Sequence order == dispatch order == turn-finish order."""
        assert_lock_held(self._lock, "DigestReorderBuffer.alloc")
        seq = self.next_seq
        self.next_seq += 1
        return seq

    def land(self, seq, apply) -> None:
        """Reorder buffer release (#88): park *apply* under *seq* and flush every
        consecutive ready slot from the serve pointer. Digests thus become
        audible strictly in dispatch order regardless of summarizer latency;
        a dropped/cancelled digest lands with apply=None and just frees its
        slot. seq=None bypasses (lead-in digests, #83: latency-critical and
        session-ordered by the question hold). Caller holds the lock. Every
        dispatched seq MUST eventually land exactly once - the workers land in
        their finally, and a hung worker's slot is landed by its watchdog
        (#138) - or later digests would park forever. A slot that was already
        served ignores a second landing (a worker returning after its
        watchdog fired).

        A release that raises is logged and the flush continues (#138, audit
        L-settle-fire): the serve pointer had already moved past it, so the
        slots parked behind it were stranded until some unrelated landing."""
        assert_lock_held(self._lock, "DigestReorderBuffer.land")
        if seq is None:
            if apply is not None:
                self._run_release(apply)
            return
        if seq < self.serve_seq:
            return                       # already served (exceptional re-land)
        t = self.watchdogs.pop(seq, None)
        if t is not None:
            t.cancel()
        self.parked[seq] = apply
        while self.serve_seq in self.parked:
            fn = self.parked.pop(self.serve_seq)
            self.serve_seq += 1
            if fn is not None:
                self._run_release(fn)

    def landed(self, seq) -> bool:
        """True once *seq* has landed: already served, or parked behind an
        earlier slot."""
        return seq < self.serve_seq or seq in self.parked

    def watch(self, seq, timer) -> None:
        """Register the hung-worker watchdog *timer* for *seq*; landing the
        slot cancels it."""
        self.watchdogs[seq] = timer

    def unwatch(self, seq) -> None:
        """Forget *seq*'s watchdog without cancelling it (the watchdog itself
        is firing)."""
        self.watchdogs.pop(seq, None)

    def kill_parked(self) -> bool:
        """Land every parked, not-yet-released digest dead in place (FLUSH_ALL):
        it already left the in-flight count, so a per-session kill cannot see
        it. Returns True when anything was killed. Caller holds the lock."""
        assert_lock_held(self._lock, "DigestReorderBuffer.kill_parked")
        killed = False
        for seq, fn in list(self.parked.items()):
            if fn is not None:
                self.parked[seq] = None    # land the slot dead
                killed = True
        return killed

    def next_release_stamp(self) -> int:
        """The next channel release stamp (#88): the router serves waiting
        digest channels lowest-stamp-first, so the heard order matches the
        release order. Caller holds the lock."""
        assert_lock_held(self._lock, "DigestReorderBuffer.next_release_stamp")
        stamp = self._release_counter
        self._release_counter += 1
        return stamp

    def _run_release(self, fn) -> None:
        try:
            fn()
        except Exception:  # noqa: BLE001 - one bad release must not strand the rest
            import traceback
            if self._log is not None:
                self._log("digest release failed:\n{0}".format(
                    traceback.format_exc()))
