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
    __slots__ = ("text", "kind", "msg_id", "heard")

    def __init__(self, text: str, kind: str, msg_id: int) -> None:
        self.text = text
        self.kind = kind          # prose|choice|plan|permission
        self.msg_id = msg_id      # message group; bumped by end_message()
        self.heard = False


class SessionHistory:
    def __init__(self, cap: int = 200) -> None:
        self._cap = cap
        self._entries: "dict[str, deque]" = {}
        self._msg_id: "dict[str, int]" = {}
        self._touch: "dict[str, int]" = {}   # recency across sessions
        self._tick = 0

    def record(self, session: str, kind: str, text: str) -> HistoryEntry:
        d = self._entries.get(session)
        if d is None:
            d = deque(maxlen=self._cap)
            self._entries[session] = d
        entry = HistoryEntry(text, kind, self._msg_id.get(session, 0))
        d.append(entry)
        self._tick += 1
        self._touch[session] = self._tick
        return entry

    def end_message(self, session: str) -> None:
        """Close the current message group (the assembler's final boundary)."""
        self._msg_id[session] = self._msg_id.get(session, 0) + 1

    def last_message(self, session: str) -> list:
        """All entries of the most recent message group (the 'whole last
        message'), oldest first."""
        d = self._entries.get(session)
        if not d:
            return []
        last_id = d[-1].msg_id
        return [e for e in d if e.msg_id == last_id]

    def unheard(self, session: str) -> list:
        """All not-yet-completed entries for session, oldest first."""
        return [e for e in self._entries.get(session, ()) if not e.heard]

    def reset(self, session: str) -> None:
        """Forget a session entirely (new prompt / session end)."""
        self._entries.pop(session, None)
        self._msg_id.pop(session, None)
        self._touch.pop(session, None)

    def other_session_with_unheard(self, exclude: str):
        """The most recently active OTHER session that has unheard entries,
        or None. Lets catch_up recover a session you left without re-typing
        in it (there is no OS window-focus hook)."""
        best, best_tick = None, -1
        for session, tick in self._touch.items():
            if session == exclude:
                continue
            if tick > best_tick and self.unheard(session):
                best, best_tick = session, tick
        return best
