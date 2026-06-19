from __future__ import annotations

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
