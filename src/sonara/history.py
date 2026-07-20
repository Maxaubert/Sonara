"""Per-session narration history + sentence-granular heard-marker.

PURE: no I/O. The Phase 2.1 substrate behind repeat / catch_up /
voice-continuity capture: every narrated-or-captured sentence is recorded per
session; `heard` flips True only when the speak loop confirms the utterance
COMPLETED, so an interrupted sentence stays unheard and a replay restarts from
the start of that sentence.
"""
from __future__ import annotations

from collections import deque


class HistoryEntry:
    __slots__ = ("text", "kind", "msg_id", "seq", "heard")

    def __init__(self, text: str, kind: str, msg_id: int, seq: int = 0) -> None:
        self.text = text
        self.kind = kind          # prose|choice|plan|permission|tool_announce|summary
        self.msg_id = msg_id      # message group; bumped by end_message()
        self.seq = seq            # 0-based index within the group; seq 0 == its head
        self.heard = False


class SessionHistory:
    def __init__(self, cap: int = 200) -> None:
        self._cap = cap
        self._entries: "dict[str, deque]" = {}
        self._msg_id: "dict[str, int]" = {}
        self._group_seq: "dict[str, int]" = {}   # next entry index within the open group
        self._touch: "dict[str, int]" = {}   # recency across sessions
        self._tick = 0

    def record(self, session: str, kind: str, text: str) -> HistoryEntry:
        d = self._entries.get(session)
        if d is None:
            d = deque(maxlen=self._cap)
            self._entries[session] = d
        seq = self._group_seq.get(session, 0)
        entry = HistoryEntry(text, kind, self._msg_id.get(session, 0), seq)
        self._group_seq[session] = seq + 1
        d.append(entry)
        self._tick += 1
        self._touch[session] = self._tick
        return entry

    def end_message(self, session: str) -> None:
        """Close the current message group (the assembler's final boundary)."""
        self._msg_id[session] = self._msg_id.get(session, 0) + 1
        self._group_seq[session] = 0          # the next group starts at the head

    def last_message(self, session: str) -> list:
        """All entries of the most recent message group (the 'whole last
        message'), oldest first."""
        d = self._entries.get(session)
        if not d:
            return []
        last_id = d[-1].msg_id
        return [e for e in d if e.msg_id == last_id]

    def message_ids(self, session: str) -> list:
        """Distinct message ids for the session, oldest first. Each id is one
        'item' (one assistant message) within the current turn; the list is the
        current turn's messages (history resets on each new prompt). Powers the
        next/prev/first/last navigation cursor."""
        d = self._entries.get(session)
        if not d:
            return []
        ids = []
        seen = set()
        for e in d:
            if e.msg_id in seen:
                continue
            seen.add(e.msg_id)
            # The first PRESENT entry of a group. If its seq != 0 the group's head
            # was evicted by the rolling cap, so the group is truncated -- exclude it
            # from navigation rather than letting nav replay a fragment (#8).
            if e.seq == 0:
                ids.append(e.msg_id)
        return ids

    def entries_for_message(self, session: str, msg_id: int) -> list:
        """All entries of a given message id, oldest first."""
        d = self._entries.get(session)
        if not d:
            return []
        return [e for e in d if e.msg_id == msg_id]

    def nth_last_message(self, session: str, n: int) -> list:
        """Entries of the n-th most recent message group, oldest first.
        n=0 is the current/most-recent message (== last_message); n=1 is the one
        before it, and so on. Returns [] if n is out of range. Powers skip-back
        ('previous item') navigation."""
        d = self._entries.get(session)
        if not d or n < 0:
            return []
        ordered_ids = []                       # distinct msg_ids, most-recent first
        for e in reversed(d):
            if e.msg_id not in ordered_ids:
                ordered_ids.append(e.msg_id)
        if n >= len(ordered_ids):
            return []
        target = ordered_ids[n]
        return [e for e in d if e.msg_id == target]

    def unheard(self, session: str) -> list:
        """All not-yet-completed entries for session, oldest first."""
        return [e for e in self._entries.get(session, ()) if not e.heard]

    def reset(self, session: str) -> None:
        """Forget a session entirely (new prompt / session end)."""
        self._entries.pop(session, None)
        self._msg_id.pop(session, None)
        self._group_seq.pop(session, None)
        self._touch.pop(session, None)

    def other_session_with_unheard(self, exclude: str, skip=None):
        """The most recently active OTHER session that has unheard entries,
        or None. Lets catch_up recover a session you left without re-typing
        in it (there is no OS window-focus hook). *skip*, when given, filters
        out sessions the caller must not surface (e.g. muted ones)."""
        best, best_tick = None, -1
        for session, tick in self._touch.items():
            if session == exclude:
                continue
            if skip is not None and skip(session):
                continue
            if tick > best_tick and self.unheard(session):
                best, best_tick = session, tick
        return best
