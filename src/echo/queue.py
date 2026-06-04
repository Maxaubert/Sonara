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

    def __len__(self) -> int:
        return len(self._items)
