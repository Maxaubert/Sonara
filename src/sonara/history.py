"""Per-session narration history + sentence-granular heard-marker.

PURE: no I/O. The substrate behind repeat and Up (restart the latest turn):
every narrated-or-captured sentence of the current turn is recorded per
session, grouped into messages; `heard` flips True only when the speak loop
confirms the utterance COMPLETED.
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

    def record(self, session: str, kind: str, text: str) -> HistoryEntry:
        d = self._entries.get(session)
        if d is None:
            d = deque(maxlen=self._cap)
            self._entries[session] = d
        seq = self._group_seq.get(session, 0)
        entry = HistoryEntry(text, kind, self._msg_id.get(session, 0), seq)
        self._group_seq[session] = seq + 1
        d.append(entry)
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
        current turn's messages (history resets on each new prompt). Up replays
        them from the first."""
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
            # from the replay rather than reading a fragment (#8).
            if e.seq == 0:
                ids.append(e.msg_id)
        return ids

    def entries_for_message(self, session: str, msg_id: int) -> list:
        """All entries of a given message id, oldest first."""
        d = self._entries.get(session)
        if not d:
            return []
        return [e for e in d if e.msg_id == msg_id]

    def reset(self, session: str) -> None:
        """Forget a session entirely (new prompt / session end)."""
        self._entries.pop(session, None)
        self._msg_id.pop(session, None)
        self._group_seq.pop(session, None)
