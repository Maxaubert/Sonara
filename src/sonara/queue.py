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
    cue_key: "str | None" = None  # coalescing key: a new cue supersedes pending/speaking cues with the same key (slider spam)
    manual: bool = False  # session_change only: armed by a manual NEXT_SESSION press -> spoken immediately in the cue voice, not deferred to content on_play (#111)
