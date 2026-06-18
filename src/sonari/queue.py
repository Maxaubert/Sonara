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
    mute_exempt: bool = False  # spoken even when the session is muted (e.g. "muted")
    pause_exempt: bool = False  # spoken even while the loop is paused (e.g. "Paused.")


class SpeechQueue:
    def __init__(self) -> None:
        self._items: "deque[SpeechItem]" = deque()

    def enqueue(self, item: SpeechItem) -> None:
        self._items.append(item)

    def enqueue_front(self, item: SpeechItem) -> None:
        """Put *item* at the head — used to resume the utterance paused mid-play."""
        self._items.appendleft(item)

    def pop_next(self) -> "SpeechItem | None":
        # The speak loop pops outside the daemon lock while flush_session can
        # swap the deque underneath it; treat a lost race as "nothing to say"
        # rather than letting an IndexError kill the speak loop (a mute daemon).
        try:
            return self._items.popleft()
        except IndexError:
            return None

    def pop_pause_exempt(self) -> "SpeechItem | None":
        """Pop the first pause-exempt item from ANYWHERE in the queue, else None.

        While paused the loop holds, but a pause confirmation ("Paused.") must still
        be voiced. It is found by scanning rather than peeking the head: a pause
        landing mid-utterance re-queues the interrupted (non-exempt) item at the
        front, so the exempt cue is not necessarily first."""
        for i, item in enumerate(self._items):
            if item.pause_exempt:
                del self._items[i]
                return item
        return None

    def jump_to_decision(self) -> "list[SpeechItem]":
        """Discard leading non-decision items so the next decision is at the front.
        Returns the discarded items so the caller can drop their heard-markers."""
        dropped = []
        while self._items and not self._items[0].is_decision:
            dropped.append(self._items.popleft())
        return dropped

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
