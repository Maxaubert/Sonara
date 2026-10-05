"""Extension namespaces (spec sections 4.2 to 4.4).

Each method sends one protocol message as is; all behaviour lives in the
runtime. A runtime that does not offer the extension (or no client enabled
it in ``hello``) answers ``E_UNSUPPORTED``. ``extra`` keyword fields are
sent along unchanged, for fields a later protocol minor adds.
"""
from __future__ import annotations

from typing import Any, Callable, Optional, Sequence, Union

Send = Callable[[str, dict], dict]
Turn = Union[str, int]


def _defined(fields: dict) -> dict:
    return {k: v for k, v in fields.items() if v is not None}


class Channels:
    """``channels``: named sources, each with its own queue and policy."""

    def __init__(self, send: Send):
        self._send = send

    def open(self, channel: str, label: Optional[str] = None, host_tab: Optional[str] = None,
             policy: Optional[str] = None, keep_label: Optional[bool] = None,
             **extra: Any) -> dict:
        """``channel_open`` (policy ``latest`` or ``queue``; ``keep_label``: an
        open channel keeps the label it has, runtime 0.20.3)."""
        return self._send("channel_open", _defined(
            {**extra, "channel": channel, "label": label, "host_tab": host_tab, "policy": policy,
             "keep_label": keep_label}))

    def close(self, channel: str, **extra: Any) -> dict:
        """``channel_close``."""
        return self._send("channel_close", _defined({**extra, "channel": channel}))

    def focus(self, channel: str, **extra: Any) -> dict:
        """``focus``: bring a channel to the front."""
        return self._send("focus", _defined({**extra, "channel": channel}))

    def speak(self, channel: str, text: str, mode: Optional[str] = None, interrupt: Optional[bool] = None,
              label: Optional[str] = None, **extra: Any) -> int:
        """``speak`` on a channel; the item id."""
        r = self._send("speak", _defined({**extra, "channel": channel, "text": text, "mode": mode,
                                          "interrupt": interrupt, "label": label}))
        return r["item_id"]

    def control(self, channel: str, action: str, **extra: Any) -> None:
        """``control`` scoped to a channel."""
        self._send("control", _defined({**extra, "channel": channel, "action": action}))

    def next_channel(self, **extra: Any) -> None:
        """``control`` ``next_channel``."""
        self._send("control", _defined({**extra, "action": "next_channel"}))

    def flush(self, **extra: Any) -> dict:
        """``control`` ``flush``: stop only the session being read (the flush hotkey, #228)."""
        return self._send("control", _defined({**extra, "action": "flush"}))


class Agent:
    """``agent`` (needs ``channels``): streaming turns, decisions, earcons."""

    def __init__(self, send: Send):
        self._send = send

    def stream(self, channel: str, turn: Turn, delta: str, index: int, final: bool,
               t: Optional[float] = None, **extra: Any) -> dict:
        """``stream``: one delta of a turn's text (``t``: sender start time)."""
        return self._send("stream", _defined({**extra, "channel": channel, "turn": turn, "delta": delta,
                                              "index": index, "final": final, "t": t}))

    def turn_start(self, channel: str, turn: Turn, **extra: Any) -> dict:
        """``turn_start``."""
        return self._send("turn_start", _defined({**extra, "channel": channel, "turn": turn}))

    def turn_end(self, channel: str, turn: Turn, **extra: Any) -> dict:
        """``turn_end``."""
        return self._send("turn_end", _defined({**extra, "channel": channel, "turn": turn}))

    def ask(self, channel: str, kind: str, text: str, options: Optional[Sequence[Any]] = None,
            **extra: Any) -> dict:
        """``ask``: ``question``, ``permission`` or ``plan``, spoken with priority."""
        return self._send("ask", _defined({**extra, "channel": channel, "kind": kind, "text": text,
                                           "options": list(options) if options is not None else None}))

    def earcon(self, kind: str, **extra: Any) -> dict:
        """``earcon``."""
        return self._send("earcon", _defined({**extra, "kind": kind}))

    def set_mute_level(self, level: int) -> dict:
        """``set mute_level`` 0, 1 or 2."""
        return self._send("set", {"key": "mute_level", "value": level})

    def set_summaries(self, value: dict) -> dict:
        """``set summaries``."""
        return self._send("set", {"key": "summaries", "value": value})


class System:
    """``system`` (Windows): other apps' audio, global hotkeys, the settings page."""

    def __init__(self, send: Send):
        self._send = send

    def set_audio_mode(self, mode: str) -> dict:
        """``set audio_mode`` (``duck``, ``pause`` or ``off``)."""
        return self._send("set", {"key": "audio_mode", "value": mode})

    def set_duck_level(self, level: int) -> dict:
        """``set duck_level``."""
        return self._send("set", {"key": "duck_level", "value": level})

    def set_hotkeys(self, hotkeys: dict) -> dict:
        """``set hotkeys``."""
        return self._send("set", {"key": "hotkeys", "value": hotkeys})

    def settings_url(self) -> str:
        """``get settings_url``."""
        return self._send("get", {"key": "settings_url"})["value"]
