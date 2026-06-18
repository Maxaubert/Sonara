"""Sonari persisted configuration: DEFAULTS plus load/save against CONFIG_PATH."""
from __future__ import annotations

import json
import os

from sonari.paths import CONFIG_PATH, SONARI_DIR, ensure_sonari_dir

DEFAULTS = {
    "voice": None,
    "rate": 200,
    "verbosity": "everything",
    "background_policy": "earcon_only",
    "history_cap": 200,
    "minqueue": 1,
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
    """Atomically persist cfg to CONFIG_PATH (temp file in SONARI_DIR + os.replace)."""
    ensure_sonari_dir()
    tmp_path = SONARI_DIR / (CONFIG_PATH.name + ".tmp")
    with open(tmp_path, "w", encoding="utf-8") as fh:
        json.dump(cfg, fh, indent=2)
        fh.flush()
        os.fsync(fh.fileno())
    os.replace(tmp_path, CONFIG_PATH)
