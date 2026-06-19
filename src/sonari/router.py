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
        self._announced: "str | None" = None   # session whose hand-off cue we emitted
        self._pending_announce: "str | None" = None

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
        self._announced = None

    def repin_reset(self) -> None:
        """On a change of pinned target, replay the pinned channel from the start."""
        pinned = self.sessions.pinned()
        if pinned is not None and pinned in self.channels:
            self.channels[pinned].reset()
        self.active = None          # force re-announce/selection
        self._announced = None

    def _ready(self, session: str) -> bool:
        ch = self.channels.get(session)
        return ch is not None and not ch.muted and ch.ready(self._minqueue())

    def _pick(self) -> "str | None":
        pinned = self.sessions.pinned()
        if pinned is not None:
            return pinned if pinned in self.channels else None
        # decisions preempt, even mid-message of another session
        for s, ch in self.channels.items():
            if ch.has_decision and self._ready(s):
                return s
        # the current reader keeps the floor while it still has a batch to read
        if self.active is not None and self._ready(self.active):
            return self.active
        # otherwise: foreground first, then oldest-waiting (insertion order)
        fg = self.sessions.foreground()
        if self._ready(fg):
            return fg
        for s in self.channels:
            if self._ready(s):
                return s
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
                if self.sessions.pinned() is None and self._announced != target:
                    self._announced = target
                    self._pending_announce = target
                    self.active = target
                    return self.next_item()   # re-enter to emit the cue first
            self.active = target
        return self.channels[target].next()
