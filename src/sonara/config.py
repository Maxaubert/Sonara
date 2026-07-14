"""Sonara persisted configuration: DEFAULTS plus load/save against CONFIG_PATH."""
from __future__ import annotations

import json
import os

from sonara.paths import CONFIG_PATH, SONARA_DIR, ensure_sonara_dir

DEFAULTS = {
    "voice": None,
    "rate": 200,
    "verbosity": "everything",
    "background_policy": "earcon_only",
    "history_cap": 200,
    "minqueue": 1,
    "audio_control": False,   # lower other apps' audio while speaking (opt-in)
    "duck_level": 30,         # target % volume for other apps while ducked (0-100)
    "summary_mode": False,    # speak an AI recap of each finished turn (opt-in)
    "summary_model": "haiku",           # model alias for the throwaway claude -p call
    "summary_command": "claude",        # executable for the summarizer subprocess
    "summary_timeout": 60,              # seconds before a summarizer call is abandoned (typical run ~12s; claude cold start adds several more)
    "chatterbox_variant": "turbo",        # default variant for voices without a sidecar
    "chatterbox_min_free_vram_gb": 5,     # VRAM gate; 0 = always try
    "chatterbox_idle_unload_s": 600,      # worker frees the model after this idle time
    "chatterbox_timeout": 120,            # seconds per-chunk synthesis worker timeout
                                          # (must cover the ~40s post-idle cold model
                                          # reload, or every post-idle chunk times out
                                          # and cascades to Kokoro -- audit #19)
    "chatterbox_warm_timeout": 90,        # seconds for a pre-warm (covers the cold load)
    "chatterbox_max_chunk_chars": 280,    # synth chunk size (80-280): smaller can
                                          # pronounce better, larger flows better (#27)
    "chatterbox_exaggeration": 0.0,       # expressiveness 0-1 (0 = monotone,
                                          # matches the turbo engine default);
                                          # a voice sidecar overrides it (#38)
    "settings_port": 27431,               # settings page port (pinned so bookmarks
                                          # and restart-reconnect work; 0 = ephemeral)
}


def _deep_merge(base: dict, override: dict) -> dict:
    """Return a new dict: override applied onto base, recursing into nested dicts."""
    result = {
        k: _deep_merge(v, {}) if isinstance(v, dict) else v
        for k, v in base.items()
    }
    for key, value in override.items():
        if (
            key in result
            and isinstance(result[key], dict)
            and isinstance(value, dict)
        ):
            result[key] = _deep_merge(result[key], value)
        else:
            result[key] = value
    return result


def load_config() -> dict:
    """Deep-merge persisted CONFIG_PATH over a copy of DEFAULTS.

    Missing or corrupt (non-JSON / non-object) files yield a fresh DEFAULTS copy.
    """
    base = _deep_merge(DEFAULTS, {})
    try:
        with open(CONFIG_PATH, "r", encoding="utf-8") as fh:
            persisted = json.load(fh)
    except (FileNotFoundError, ValueError, OSError):
        return base
    if not isinstance(persisted, dict):
        return base
    return _deep_merge(base, persisted)


def save_config(cfg: dict) -> None:
    """Atomically persist cfg to CONFIG_PATH (temp file in SONARA_DIR + os.replace)."""
    ensure_sonara_dir()
    tmp_path = SONARA_DIR / (CONFIG_PATH.name + ".tmp")
    with open(tmp_path, "w", encoding="utf-8") as fh:
        json.dump(cfg, fh, indent=2)
        fh.flush()
        os.fsync(fh.fileno())
    os.replace(tmp_path, CONFIG_PATH)
