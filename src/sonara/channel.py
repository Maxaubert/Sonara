"""One session's current message: an item list + a read cursor.

Items are NOT discarded as they are spoken -- the cursor advances over them -- so a
channel can resume from where it left off (auto hand-off) or replay from the start
(session-change revisit). A new prompt wipes the channel.
"""
from __future__ import annotations

from sonara.queue import SpeechItem


class SessionChannel:
    def __init__(self, session: str) -> None:
        self.session = session
        self.items: "list[SpeechItem]" = []
        self.cursor = 0
        self.turn_done = False
        self.muted = False
        self.gen = 0        # content generation: bumped on append AND wipe. "Has
        #                     this channel changed since X?" checks key on this,
        #                     NOT on len(items): in summary mode every turn is
        #                     wipe(->0)+append(->1), landing back on the same
        #                     length, which left length-based router suppression
        #                     stuck forever (#115).
        self.replaying = False  # a manual replay is in progress: re-landing
        #                         restarts from the top instead of resuming
        #                         mid-message (#118); cleared by new content
        self.seeded = False     # items are a persisted-digest placeholder that
        #                         keeps the session cycle-reachable while its
        #                         turn cooks (#118); real content replaces it
        self.has_decision = False   # a user-blocking item is pending -> preempt
        self.release_order = None   # digest release stamp (#88): among READY
        #                             waiting sessions the router serves the
        #                             lowest stamp first, so digests are heard
        #                             in turn-finish order, not channel order

    def append(self, item: SpeechItem) -> None:
        if self.seeded:
            self.wipe()   # real content replaces the placeholder seed (#118)
        self.items.append(item)
        self.gen += 1
        self.replaying = False
        if item.is_decision:
            self.has_decision = True

    def pending(self) -> int:
        return len(self.items) - self.cursor

    def ready(self, minqueue: int) -> bool:
        """True if there is a batch worth reading now: enough buffered, the turn is
        done, or a user-blocking decision is waiting."""
        p = self.pending()
        return p > 0 and (p >= minqueue or self.turn_done or self.has_decision)

    def caught_up(self) -> bool:
        return self.cursor >= len(self.items)

    def peek(self) -> "SpeechItem | None":
        return self.items[self.cursor] if self.cursor < len(self.items) else None

    def take_pause_exempt(self) -> "SpeechItem | None":
        """Remove and return the first pause_exempt item at/after the cursor (the
        confirmation cue), so it can be spoken while the loop is held even if a
        cursor-rewind left it just past the cursor. Returns None if there is none."""
        for i in range(self.cursor, len(self.items)):
            if self.items[i].pause_exempt:
                return self.items.pop(i)
        return None

    def next(self) -> "SpeechItem | None":
        if self.cursor >= len(self.items):
            return None
        item = self.items[self.cursor]
        self.cursor += 1
        if self.caught_up():
            self.has_decision = False   # the pending decision has been consumed
        return item

    def reset(self) -> None:
        self.cursor = 0

    def wipe(self) -> None:
        self.items = []
        self.cursor = 0
        self.turn_done = False
        self.has_decision = False
        self.gen += 1
        self.replaying = False
        self.seeded = False
