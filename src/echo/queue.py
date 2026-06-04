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
        if not self._items:
            return None
        return self._items.popleft()

    def jump_to_decision(self) -> None:
        while self._items and not self._items[0].is_decision:
            self._items.popleft()

    def clear(self) -> None:
        self._items.clear()

    def flush_session(self, session: str) -> None:
        self._items = deque(
            item for item in self._items if item.session != session
        )

    def __len__(self) -> int:
        return len(self._items)
