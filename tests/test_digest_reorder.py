"""DigestReorderBuffer (#88, moved out of the daemon in #141): digests release
in dispatch order whatever order their workers finish in."""
import threading

import pytest

from sonara.daemon import core
from sonara.daemon.summary.reorder import DigestReorderBuffer


def test_releases_in_dispatch_order_not_landing_order():
    buf, ran = DigestReorderBuffer(), []
    s0, s1, s2 = buf.alloc(), buf.alloc(), buf.alloc()
    buf.land(s2, lambda: ran.append(2))
    buf.land(s1, lambda: ran.append(1))
    assert ran == [] and buf.landed(s2) and not buf.landed(s0)
    buf.land(s0, lambda: ran.append(0))
    assert ran == [0, 1, 2]
    assert buf.parked == {} and buf.serve_seq == buf.next_seq == 3


def test_seq_none_bypasses_the_order():
    buf, ran = DigestReorderBuffer(), []
    buf.alloc()                                   # an earlier slot still out
    buf.land(None, lambda: ran.append("leadin"))
    assert ran == ["leadin"]


def test_kill_parked_lands_waiting_digests_dead():
    buf, ran = DigestReorderBuffer(), []
    s0, s1 = buf.alloc(), buf.alloc()
    buf.land(s1, lambda: ran.append(1))
    assert buf.kill_parked() is True
    assert buf.kill_parked() is False             # nothing left alive
    buf.land(s0, None)
    assert ran == [] and buf.parked == {}


def test_a_second_landing_of_a_served_slot_is_ignored():
    buf, ran = DigestReorderBuffer(), []
    s0 = buf.alloc()
    buf.land(s0, lambda: ran.append("first"))
    buf.land(s0, lambda: ran.append("again"))
    assert ran == ["first"]


def test_landing_cancels_the_slot_watchdog_and_unwatch_does_not():
    class _Timer:
        cancelled = False

        def cancel(self):
            self.cancelled = True

    buf = DigestReorderBuffer()
    s0, s1 = buf.alloc(), buf.alloc()
    t0, t1 = _Timer(), _Timer()
    buf.watch(s0, t0)
    buf.watch(s1, t1)
    buf.land(s0, None)
    buf.unwatch(s1)
    assert t0.cancelled and not t1.cancelled and buf.watchdogs == {}


def test_a_raising_release_is_logged_and_later_slots_still_run():
    logged, ran = [], []
    buf = DigestReorderBuffer(log=logged.append)
    s0, s1 = buf.alloc(), buf.alloc()
    buf.land(s1, lambda: ran.append(1))
    buf.land(s0, lambda: (_ for _ in ()).throw(RuntimeError("boom")))
    assert ran == [1]
    assert logged and "digest release failed" in logged[0]


def test_release_stamps_increase():
    buf = DigestReorderBuffer()
    assert [buf.next_release_stamp() for _ in range(3)] == [0, 1, 2]


def test_debug_lock_check_flags_a_caller_without_the_lock(monkeypatch):
    monkeypatch.setattr(core, "CHECK_LOCKS", True)
    lock = threading.Lock()
    buf = DigestReorderBuffer(lock=lock)
    with pytest.raises(AssertionError):
        buf.alloc()
    with lock:
        assert buf.alloc() == 0
