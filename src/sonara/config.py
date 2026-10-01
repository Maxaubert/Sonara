"""Sonara persisted configuration: DEFAULTS plus load/save against CONFIG_PATH.

The defaults and validators live in config_schema.py. config.json holds only
the keys the user set, so new defaults reach existing installs (#136, M7).
"""
from __future__ import annotations

import json
import os

from sonara import config_schema
from sonara.paths import APP_DIR, CONFIG_PATH, SONARA_DIR, ensure_sonara_dir

DEFAULTS = config_schema.defaults()

# Optional user keys with no default: earcons maps kind -> a user's own wav.
# The bundled earcons are resolved at runtime, never stored (M8).
OVERRIDE_KEYS = ("earcons",)

# Written into config.json by this version. A file without it was written by
# a pre-#136 save_config, which dumped every key (see _legacy_unset).
FORMAT_KEY = "_format"
FORMAT_VERSION = 2


class Config(dict):
    """The loaded config: a dict that remembers which keys the user set.

    `explicit` holds the keys read from config.json plus every key assigned
    since (cfg[key] = value is how every setter changes a value), so
    save_config persists those and nothing else. A user who sets a value equal
    to today's default keeps it if the default later moves; a key never set
    follows the default (M7).
    """

    def __init__(self, data=(), explicit=()):
        super().__init__(data)
        self.explicit = set(explicit)

    def __setitem__(self, key, value):
        super().__setitem__(key, value)
        self.explicit.add(key)

    def __delitem__(self, key):
        super().__delitem__(key)
        self.explicit.discard(key)

    def pop(self, key, *default):
        self.explicit.discard(key)
        return super().pop(key, *default)


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


def _legacy_unset(key, value) -> bool:
    """True when a pre-#136 file's value is just a default it was written
    with. Those files hold every key, so a value equal to the current or a
    past default (config_schema.LEGACY_DEFAULTS) cannot be told apart from
    "never touched" and is treated as unset: the user then follows today's
    default. A value the user really chose differs from every default."""
    if key not in DEFAULTS:
        return False
    return (value == DEFAULTS[key]
            or value in config_schema.LEGACY_DEFAULTS.get(key, ()))


def _is_bundled_earcon(path) -> bool:
    """A path to one of Sonara's own earcon wavs: inside the deployed app
    copy, or in the package's earcons folder of any copy (repo, plugin cache).
    Older versions froze these into config.json (M8)."""
    try:
        p = os.path.normcase(os.path.abspath(str(path)))
    except (TypeError, ValueError):
        return False
    app = os.path.normcase(os.path.abspath(str(APP_DIR)))
    if p == app or p.startswith(app + os.sep):
        return True
    tail = os.path.normcase(os.path.join("sonara", "platform", "windows", "earcons"))
    return os.path.dirname(p).endswith(os.sep + tail)


def _user_earcons(value):
    """The user's own earcon overrides, minus bundled paths; None if empty."""
    if not isinstance(value, dict):
        return None
    kept = {k: v for k, v in value.items()
            if isinstance(v, str) and v and not _is_bundled_earcon(v)}
    return kept or None


def load_config() -> Config:
    """Deep-merge persisted CONFIG_PATH over a copy of DEFAULTS.

    Missing or corrupt (non-JSON / non-object) files yield a fresh DEFAULTS
    copy. Unknown keys are dropped. The keys kept from the file are the
    user's explicit settings (Config.explicit).
    """
    base = _deep_merge(DEFAULTS, {})
    try:
        with open(CONFIG_PATH, "r", encoding="utf-8") as fh:
            persisted = json.load(fh)
    except (FileNotFoundError, ValueError, OSError):
        return Config(base)
    if not isinstance(persisted, dict):
        return Config(base)
    legacy = persisted.pop(FORMAT_KEY, None) is None
    # Migrate the pre-#92 boolean into the three-way mode when the persisted file
    # predates audio_mode: audio_control True -> "duck", otherwise the default "off".
    # The key itself is gone, so drop it and the next save stops persisting it.
    if "audio_mode" not in persisted and persisted.get("audio_control"):
        persisted["audio_mode"] = "duck"
    persisted.pop("audio_control", None)
    # Chatterbox was removed (#134): a saved Chatterbox voice speaks as Heart,
    # and its old settings are dropped so the next save stops persisting them.
    from sonara import chatterbox_legacy
    chatterbox_legacy.migrate_config(persisted)
    earcons = _user_earcons(persisted.pop("earcons", None))
    user = {k: v for k, v in persisted.items()
            if k in DEFAULTS and not (legacy and _legacy_unset(k, v))}
    if earcons:
        user["earcons"] = earcons
    return Config(_deep_merge(base, user), explicit=user)


def _to_persist(cfg: dict) -> dict:
    """The keys of cfg worth writing: explicit settings (when cfg is a loaded
    Config) and every value that differs from its default. Never unknown keys
    and never bundled earcon paths."""
    explicit = getattr(cfg, "explicit", set())
    out = {}
    for key, value in cfg.items():
        if key in OVERRIDE_KEYS:
            if key == "earcons":
                value = _user_earcons(value)
                if value:
                    out[key] = value
            continue
        if key not in DEFAULTS:
            continue
        if key in explicit or value != DEFAULTS[key]:
            out[key] = value
    return out


def save_config(cfg: dict) -> None:
    """Atomically persist the user's settings in cfg to CONFIG_PATH (temp file
    in SONARA_DIR + os.replace). Keys still at their default are left out."""
    data = _to_persist(cfg)
    data[FORMAT_KEY] = FORMAT_VERSION
    ensure_sonara_dir()
    tmp_path = SONARA_DIR / (CONFIG_PATH.name + ".tmp")
    with open(tmp_path, "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2)
        fh.flush()
        os.fsync(fh.fileno())
    os.replace(tmp_path, CONFIG_PATH)
