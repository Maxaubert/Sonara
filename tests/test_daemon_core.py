"""Debug-only lock-discipline check (#141): code that assumes the caller holds
the daemon lock asserts it when SONARA_DEBUG_LOCKS is on, and is a no-op
otherwise."""
import threading

import pytest

from sonara.daemon import core


def test_lock_check_is_off_by_default_and_never_raises(monkeypatch):
    monkeypatch.setattr(core, "CHECK_LOCKS", False)
    core.assert_lock_held(threading.Lock(), "free lock, checks off")


def test_lock_check_raises_on_a_free_lock_when_enabled(monkeypatch):
    monkeypatch.setattr(core, "CHECK_LOCKS", True)
    with pytest.raises(AssertionError, match="land"):
        core.assert_lock_held(threading.Lock(), "land")


def test_lock_check_passes_while_the_lock_is_held(monkeypatch):
    monkeypatch.setattr(core, "CHECK_LOCKS", True)
    lock = threading.Lock()
    with lock:
        core.assert_lock_held(lock, "held")
