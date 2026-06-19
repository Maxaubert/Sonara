"""Choose the single active reader among per-session channels and yield the next
item to speak. One speaker -> one reader at a time. See the design spec."""
from __future__ import annotations

from sonari.channel import SessionChannel
from sonari.queue import SpeechItem


class Router:
    def __init__(self, sessions, minqueue, announce_text) -> None:
        self.sessions = sessions          # exposes pinned()/foreground()/folder()
        self._minqueue = minqueue          # () -> int
        self._announce_text = announce_text  # (folder) -> str
        self.channels: "dict[str, SessionChannel]" = {}
        self.active: "str | None" = None
        self._announced: "set[str]" = set()   # sessions whose hand-off cue has been emitted
        self._pending_announce: "str | None" = None
        # Sessions explicitly authorized for reading even when not fg/pinned
        # (set by the daemon for catch_up/replay cross-session scenarios, and
        # for old-fg drains when the foreground switches mid-response).
        self._speakable: "set[str]" = set()

    def channel(self, session: str) -> SessionChannel:
        ch = self.channels.get(session)
        if ch is None:
            ch = SessionChannel(session)
            self.channels[session] = ch
        return ch

    def drop(self, session: str) -> None:
        self.channels.pop(session, None)
        if self._pending_announce == session:
            self._pending_announce = None
        if self.active == session:
            self.active = None
        self._announced = set()
        self._speakable.discard(session)

    def repin_reset(self) -> None:
        """On a change of pinned target, replay the pinned channel from the start."""
        pinned = self.sessions.pinned()
        if pinned is not None and pinned in self.channels:
            self.channels[pinned].reset()
        self.active = None          # force re-announce/selection
        self._announced = set()

    def _ready(self, session: str) -> bool:
        ch = self.channels.get(session)
        if ch is None:
            return False
        if ch.muted:
            # A mute-exempt cue (e.g. "Session muted." / "Session unmuted.") at
            # the cursor must still be spoken even though the channel is muted.
            peeked = ch.peek()
            return peeked is not None and peeked.mute_exempt
        return ch.ready(self._minqueue())

    def _pick(self) -> "str | None":
        pinned = self.sessions.pinned()
        if pinned is not None:
            return pinned if pinned in self.channels else None
        # decisions preempt mid-message — but only when there is already an
        # active reader (i.e. we are mid-batch). If active is None, the router
        # checks fg first; a cue inserted at fg's cursor takes priority over a
        # background session's decision.
        if self.active is not None:
            for s, ch in self.channels.items():
                if ch.has_decision and self._ready(s):
                    return s
        # the current reader keeps the floor until its channel is empty: once a
        # batch starts (threshold or turn_done triggered _ready), keep reading
        # all items in that channel before switching to another session.
        if self.active is not None:
            ch = self.channels.get(self.active)
            if ch is not None and ch.pending() > 0:
                # Only keep the floor if the batch is still valid (not muted
                # with no exempt item, which would stall indefinitely).
                if not ch.muted or self._ready(self.active):
                    return self.active
        # Speakable sessions drain first: sessions explicitly authorized for
        # cross-session reading (e.g. old-fg draining after a session switch,
        # catch_up replay targets). Pre-suppress the announcement for both the
        # speakable session itself and the fg we'll transition to next, since
        # this is a natural hand-off, not a user-visible session switch.
        for s in list(self._speakable):
            if self._ready(s):
                self._announced.add(s)          # suppress s announcing itself
                fg_now = self.sessions.foreground()
                if fg_now is not None:
                    self._announced.add(fg_now) # suppress fg announcing after s drains
                return s
            # Auto-evict exhausted speakable sessions so they don't linger.
            ch = self.channels.get(s)
            if ch is None or ch.pending() == 0:
                self._speakable.discard(s)
        # Otherwise: foreground first.
        fg = self.sessions.foreground()
        if self._ready(fg):
            return fg
        return None

    def next_item(self) -> "SpeechItem | None":
        # emit a queued hand-off announcement before the new reader's first item
        if self._pending_announce is not None:
            folder = self.sessions.folder(self._pending_announce) or "another session"
            text = self._announce_text(folder)
            self._pending_announce = None
            return SpeechItem(id=0, session=self.active or "", kind="prose",
                              text=text, is_decision=False, mute_exempt=True)
        target = self._pick()
        if target is None:
            self.active = None
            return None
        if target != self.active:
            # if we had a previous reader and it wasn't None, we're switching readers
            if self.active is not None:
                # announce in auto mode only, once per becoming-active
                if self.sessions.pinned() is None and target not in self._announced:
                    self._announced.add(target)
                    self._pending_announce = target
                    self.active = target
                    return self.next_item()   # re-enter to emit the cue first
            self.active = target
        return self.channels[target].next()
