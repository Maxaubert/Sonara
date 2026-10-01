"""Shared daemon plumbing: the lock-discipline check and the state several
features read (SharedState). The rest of the core state (_lock, _wake, ids,
mute) moves here in a later step of the split.

Much of the daemon runs on the rule "the caller holds the daemon lock", which
nothing enforced. Code that relies on it calls assert_lock_held(). The check
is off by default, so it never changes how the daemon runs; set
SONARA_DEBUG_LOCKS=1 to turn it on when diagnosing a lock-discipline bug."""
from __future__ import annotations

import os

CHECK_LOCKS = os.environ.get("SONARA_DEBUG_LOCKS") == "1"


def assert_lock_held(lock, what: str = "") -> None:
    """Raise AssertionError when the lock checks are on and *lock* is free.
    threading.Lock cannot say WHICH thread holds it, so this catches a caller
    that forgot the lock entirely, not one racing another holder."""
    if CHECK_LOCKS and lock is not None and not lock.locked():
        raise AssertionError(
            "daemon lock not held{0}".format(": " + what if what else ""))


class SharedState:
    """State that more than one daemon feature reads or writes, so it belongs
    to none of them. Guarded by the daemon lock, like everything it replaces.

    *current_item* is the item being spoken right now (the speak loop sets it;
    FLUSH, SKIP, Up, flush-to-end and the cue path read it).
    *last_digest_text* maps a session to the exact text Up re-reads in summary
    mode: the summary pipeline sets it, note_spoken appends heard questions,
    and the re-read speaks it verbatim so cached audio replays."""

    def __init__(self) -> None:
        self.current_item = None
        self.last_digest_text: dict = {}
