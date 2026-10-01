"""The one table of Sonara settings (#136): default, validator and live-apply
hook per config.json key.

config.DEFAULTS is derived from it; the daemon's SET_* handlers and
set_config_value, the settings page key tables and the CLI choices all read it,
so a default or a clamp lives in exactly one place.

A validator ("clean") takes the raw value and returns the value to store. It
returns None or raises TypeError/ValueError to reject the value. A setting
with no validator is stored verbatim.
"""
from __future__ import annotations

import copy
from typing import Any, Callable, Dict, NamedTuple, Optional, Tuple

RATE_MIN = 100
RATE_MAX = 400
# Min-queue batching: how many prose items must accumulate before they are read.
# 1 == read each item as it arrives (the default, unchanged behaviour).
MINQUEUE_MIN = 0     # 0 = start reading immediately, no batching (#60 follow-up)
MINQUEUE_MAX = 10

VERBOSITY_CHOICES = ("everything", "medium", "quiet")
AUDIO_MODES = ("off", "duck", "pause")
SUMMARY_STYLES = ("tidy", "natural", "brief")
SUMMARY_COMMANDS = ("claude", "codex")


def clamp_int(lo: int, hi: int) -> Callable[[Any], int]:
    return lambda v: max(lo, min(hi, int(v)))


def one_of(*options: str) -> Callable[[Any], Optional[str]]:
    return lambda v: str(v) if str(v) in options else None


def _non_blank(v: Any) -> Optional[str]:
    return str(v).strip() or None


class Setting(NamedTuple):
    default: Any
    clean: Optional[Callable[[Any], Any]] = None
    # Settings page: "msg" keys go through a protocol message (the same path
    # as the CLI and hotkeys); "config" keys go through set_config_value.
    page: Optional[str] = None
    # protocol message builder for a "msg" page key
    message: Optional[Callable[[Any], dict]] = None
    # SpeechDaemon method called after set_config_value stores the key
    apply: Optional[str] = None
    choices: Tuple[str, ...] = ()


SCHEMA: Dict[str, Setting] = {
    "voice": Setting(
        None, page="msg",
        message=lambda v: {"type": "set_voice", "voice": str(v)}),
    "rate": Setting(
        200, clamp_int(RATE_MIN, RATE_MAX), page="msg",
        message=lambda v: {"type": "set_rate", "rate": int(v)}),
    # stored verbatim (the CLI restricts it to the choices)
    "verbosity": Setting("everything", choices=VERBOSITY_CHOICES),
    "background_policy": Setting("earcon_only"),
    "history_cap": Setting(200),
    "minqueue": Setting(
        1, clamp_int(MINQUEUE_MIN, MINQUEUE_MAX), page="msg",
        message=lambda v: {"type": "set_minqueue", "minqueue": int(v)}),
    # target % volume for other apps while ducked
    "duck_level": Setting(
        30, clamp_int(0, 100), page="msg",
        message=lambda v: {"type": "set_duck_level", "level": int(v)}),
    # speech gain percent
    "volume": Setting(
        100, clamp_int(25, 200), page="msg",
        message=lambda v: {"type": "set_volume", "volume": int(v)}),
    # off | duck | pause: pause pauses SMTC media (#92)
    "audio_mode": Setting(
        "off", one_of(*AUDIO_MODES), page="msg",
        message=lambda v: {"type": "set_audio_mode", "mode": str(v)},
        choices=AUDIO_MODES),
    # speak an AI recap of each finished turn (opt-in)
    "summary_mode": Setting(
        False, bool, page="msg",
        message=lambda v: {"type": "set_summary_mode", "enabled": bool(v)}),
    # model alias for the throwaway claude -p call
    "summary_model": Setting("haiku", _non_blank, page="config"),
    # executable for the summarizer subprocess
    "summary_command": Setting(
        "claude", one_of(*SUMMARY_COMMANDS), page="config",
        choices=SUMMARY_COMMANDS),
    # seconds before a summarizer call is abandoned (typical run ~12s; claude
    # cold start adds several more)
    "summary_timeout": Setting(60, clamp_int(15, 300), page="config"),
    # quiet time after a turn ends before its digest is requested
    "summary_settle_ms": Setting(600, clamp_int(0, 5000), page="config"),
    "summary_style": Setting(
        "natural", one_of(*SUMMARY_STYLES), page="config",
        choices=SUMMARY_STYLES),
    # style -> custom instruction; absent = default (set_summary_prompt)
    "summary_prompts": Setting({}),
    # control cues speak via an always-fast voice, never waiting out a cold
    # neural model reload (#60)
    "fast_cues": Setting(
        True, bool, page="config", apply="_maybe_prewarm_cue_voice"),
    # which fast voice speaks the cues: a Kokoro voice (engine kept warm,
    # ~0.3s) or a native Windows voice (#60)
    "cue_voice": Setting(
        "af_heart", _non_blank, page="config",
        apply="_maybe_prewarm_cue_voice"),
    # persisted mute cycle (0/1/2): hooks silently respawn a dead daemon, so
    # a memory-only mute reset itself between two messages (#65)
    "mute_level": Setting(0, clamp_int(0, 2)),
    # settings page port (pinned so bookmarks and restart-reconnect work;
    # 0 = ephemeral)
    "settings_port": Setting(27431),
}

# Defaults that earlier releases shipped. Before #136 save_config wrote every
# key, so an old config.json holds these values even where the user never
# touched them; config.load_config treats them as unset in such a file.
LEGACY_DEFAULTS: Dict[str, Tuple[Any, ...]] = {
    "duck_level": (20,),
    "summary_timeout": (20,),
}

INVALID = object()   # clean() result for a rejected value


def defaults() -> dict:
    """A fresh {key: default} dict (nested defaults are copies)."""
    return {k: copy.deepcopy(s.default) for k, s in SCHEMA.items()}


def default(key: str) -> Any:
    return copy.deepcopy(SCHEMA[key].default)


def clean(key: str, value: Any) -> Any:
    """The value to store for *key*, or INVALID when it is rejected."""
    fn = SCHEMA[key].clean
    if fn is None:
        return value
    try:
        out = fn(value)
    except (TypeError, ValueError):
        return INVALID
    return INVALID if out is None else out


def get(cfg: dict, key: str) -> Any:
    """cfg[key], or the schema default when the key is missing."""
    return cfg[key] if key in cfg else default(key)


def current(cfg: dict, key: str) -> Any:
    """cfg[key] cleaned, falling back to the schema default when the value is
    missing or invalid (a hand-edited config.json can hold anything)."""
    if key not in cfg:
        return default(key)
    out = clean(key, cfg[key])
    return default(key) if out is INVALID else out


def page_keys() -> Tuple[str, ...]:
    return tuple(k for k, s in SCHEMA.items() if s.page)


def page_messages() -> Dict[str, Callable[[Any], dict]]:
    return {k: s.message for k, s in SCHEMA.items() if s.page == "msg"}


def config_only_keys() -> Tuple[str, ...]:
    return tuple(k for k, s in SCHEMA.items() if s.page == "config")
