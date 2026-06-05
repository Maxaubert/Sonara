"""Sonari Phase 2 keymap: ALL hotkey logic lives here (the Swift binary is dumb).

Maps key names -> macOS virtual key codes, modifier names -> Carbon masks, and
actions -> speechd protocol messages. Produces the resolved JSON array that the
Swift hotkeyd reads, registers, and sends on fire.
"""

import json
import os

from sonari.paths import (
    KEYMAP_PATH,
    HOTKEYD_RESOLVED_PATH,
    SONARI_DIR,
    ensure_sonari_dir,
)

# macOS ANSI virtual key codes (Carbon kVK_ANSI_*).
KEY_CODES = {
    "s": 1,
    "r": 15,
    "d": 2,
    "l": 37,
    "v": 9,
    "o": 31,
    "period": 47,
    ".": 47,
    "rightbracket": 30,
    "]": 30,
    "leftbracket": 33,
    "[": 33,
}

# Carbon modifier masks.
MOD_MASKS = {
    "cmd": 256,
    "shift": 512,
    "opt": 2048,
    "option": 2048,
    "ctrl": 4096,
    "control": 4096,
}

# action -> the speechd protocol message it sends.
ACTION_MESSAGES = {
    "stop": {"type": "stop"},
    "repeat": {"type": "repeat"},
    "skip": {"type": "skip"},
    "jump_decision": {"type": "jump_decision"},
    "catch_up": {"type": "catch_up"},
    "faster": {"type": "set_rate", "delta": 25},
    "slower": {"type": "set_rate", "delta": -25},
    "cycle_verbosity": {"type": "cycle_verbosity"},
    "reread_options": {"type": "reread_options"},
}

# Default bindings (modifier Ctrl+Cmd, chosen to avoid VoiceOver's Ctrl+Opt).
DEFAULT_KEYMAP = {
    "stop": {"key": "s", "mods": ["ctrl", "cmd"]},
    "repeat": {"key": "r", "mods": ["ctrl", "cmd"]},
    "skip": {"key": ".", "mods": ["ctrl", "cmd"]},
    "jump_decision": {"key": "d", "mods": ["ctrl", "cmd"]},
    "catch_up": {"key": "l", "mods": ["ctrl", "cmd"]},
    "faster": {"key": "]", "mods": ["ctrl", "cmd"]},
    "slower": {"key": "[", "mods": ["ctrl", "cmd"]},
    "cycle_verbosity": {"key": "v", "mods": ["ctrl", "cmd"]},
    "reread_options": {"key": "o", "mods": ["ctrl", "cmd"]},
}


def _copy_keymap(km: dict) -> dict:
    """Deep-ish copy: each action maps to a fresh {key, mods[...]} dict."""
    out = {}
    for action, binding in km.items():
        out[action] = {
            "key": binding.get("key"),
            "mods": list(binding.get("mods", [])),
        }
    return out


def resolve_keymap(keymap=None) -> list:
    """Resolve an action->binding map into the Swift-facing array.

    Each output entry: {action, keyCode, modifiers, message}. Raises ValueError
    on an unknown key name, unknown modifier name, or unknown action.
    """
    if keymap is None:
        keymap = DEFAULT_KEYMAP
    resolved = []
    for action, binding in keymap.items():
        if action not in ACTION_MESSAGES:
            raise ValueError("unknown action: {0}".format(action))
        key = (binding.get("key") or "").lower()
        if key not in KEY_CODES:
            raise ValueError("unknown key: {0}".format(binding.get("key")))
        mask = 0
        for mod in binding.get("mods", []):
            m = (mod or "").lower()
            if m not in MOD_MASKS:
                raise ValueError("unknown modifier: {0}".format(mod))
            mask |= MOD_MASKS[m]
        resolved.append({
            "action": action,
            "keyCode": KEY_CODES[key],
            "modifiers": mask,
            "message": json.dumps(ACTION_MESSAGES[action]),
        })
    return resolved


def load_keymap() -> dict:
    """Merge the user's KEYMAP_PATH over a copy of DEFAULT_KEYMAP.

    Missing or corrupt files yield a fresh DEFAULT_KEYMAP copy. A user entry
    fully replaces the default binding for that action.
    """
    merged = _copy_keymap(DEFAULT_KEYMAP)
    try:
        with open(KEYMAP_PATH, "r", encoding="utf-8") as fh:
            user = json.load(fh)
    except (FileNotFoundError, ValueError, OSError):
        return merged
    if not isinstance(user, dict):
        return merged
    for action, binding in user.items():
        if isinstance(binding, dict):
            merged[action] = {
                "key": binding.get("key"),
                "mods": list(binding.get("mods", [])),
            }
    return merged


def write_default_keymap_if_absent() -> bool:
    """Write DEFAULT_KEYMAP to KEYMAP_PATH if it does not exist. Returns True
    iff it wrote the file."""
    if os.path.exists(KEYMAP_PATH):
        return False
    ensure_sonari_dir()
    with open(KEYMAP_PATH, "w", encoding="utf-8") as fh:
        json.dump(DEFAULT_KEYMAP, fh, indent=2)
        fh.flush()
        os.fsync(fh.fileno())
    return True


def write_resolved(keymap=None) -> str:
    """Atomically write the resolved array to HOTKEYD_RESOLVED_PATH; return its
    path. Uses load_keymap() when no explicit keymap is given."""
    if keymap is None:
        keymap = load_keymap()
    data = json.dumps(resolve_keymap(keymap))
    ensure_sonari_dir()
    tmp_path = SONARI_DIR / (HOTKEYD_RESOLVED_PATH.name + ".tmp")
    with open(tmp_path, "w", encoding="utf-8") as fh:
        fh.write(data)
        fh.flush()
        os.fsync(fh.fileno())
    os.replace(tmp_path, HOTKEYD_RESOLVED_PATH)
    return str(HOTKEYD_RESOLVED_PATH)
