from __future__ import annotations

from collections import deque
from dataclasses import dataclass


@dataclass
class SpeechItem:
    id: int
    session: str
    kind: str          # one of prose|choice|plan|permission|tool_announce
    text: str
    is_decision: bool  # True for choice|plan|permission


class SpeechQueue:
    def __init__(self) -> None:
        self._items: "deque[SpeechItem]" = deque()

    def enqueue(self, item: SpeechItem) -> None:
        self._items.append(item)

    def pop_next(self) -> "SpeechItem | None":
        # The speak loop pops outside the daemon lock while flush_session can
        # swap the deque underneath it; treat a lost race as "nothing to say"
        # rather than letting an IndexError kill the speak loop (a mute daemon).
        try:
            return self._items.popleft()
        except IndexError:
            return None

    def jump_to_decision(self) -> None:
        while self._items and not self._items[0].is_decision:
            self._items.popleft()

    def clear(self) -> "list[SpeechItem]":
        dropped = list(self._items)
        self._items.clear()
        return dropped

    def flush_session(self, session: str) -> "list[SpeechItem]":
        dropped = [i for i in self._items if i.session == session]
        self._items = deque(
            item for item in self._items if item.session != session
        )
        return dropped

    def __len__(self) -> int:
        return len(self._items)
