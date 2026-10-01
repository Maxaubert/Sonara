"""Shared daemon plumbing: the lock-discipline check, the state several
features read (SharedState) and the per-session state registry
(SessionRegistry). The lock, wake and paused events, item ids and the mute
level stay on SpeechDaemon, which every message handler is given.

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


def add_handlers(table: dict, handlers: dict) -> None:
    """Add a feature's message handlers to the daemon's dispatch table (#141).
    Each message type has exactly one owner: registering one twice is a
    wiring bug, so it raises instead of silently replacing the first."""
    for mtype, handler in handlers.items():
        if mtype in table:
            raise ValueError("message type {0!r} has two handlers".format(mtype))
        table[mtype] = handler


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


class SessionRegistry:
    """Everything the daemon keeps per session, registered by the feature
    that owns it (#141). Ending or forgetting a session calls
    forget_session(sid) once instead of a hand-written list of pops, so a
    new per-session dict cannot be left out of the teardown (a stale entry
    outlived its session more than once, audit #19, #21).

    A store is a dict keyed by session (popped) or a set of sessions
    (discarded). A hook is a callable taking the session id, for state that
    needs more than a pop (bumping a cancel epoch, dropping heard-markers).
    forget_session runs both in registration order. Guarded by the daemon
    lock, like the state it clears."""

    def __init__(self) -> None:
        self._entries: list = []      # (name, forget(sid), store or None)

    def register(self, name: str, store) -> None:
        """Register a per-session dict or set under *name*."""
        if isinstance(store, dict):
            forget = (lambda sid, _s=store: _s.pop(sid, None))
        elif isinstance(store, set):
            forget = store.discard
        else:
            raise TypeError("per-session store {0!r} must be a dict or a "
                            "set".format(name))
        self._add(name, forget, store)

    def register_hook(self, name: str, forget) -> None:
        """Register a callable that clears *name*'s state for one session."""
        if not callable(forget):
            raise TypeError("per-session hook {0!r} is not callable".format(name))
        self._add(name, forget, None)

    def _add(self, name: str, forget, store) -> None:
        if name in self.names():
            raise ValueError("per-session state {0!r} registered "
                             "twice".format(name))
        self._entries.append((name, forget, store))

    def names(self) -> list:
        return [name for name, _f, _s in self._entries]

    def stores(self) -> list:
        """(name, container) for every registered dict and set."""
        return [(name, s) for name, _f, s in self._entries if s is not None]

    def forget_session(self, session: str) -> None:
        for _name, forget, _store in self._entries:
            forget(session)
