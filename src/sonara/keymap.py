"""Sonara keymap: ALL hotkey logic lives here.

Maps key names -> Windows virtual key codes, modifier names -> RegisterHotKey
modifier masks, and actions -> speechd protocol messages. Produces the resolved
list that the daemon's in-process hotkey listener registers and fires on.
"""
from __future__ import annotations

import json
import os

from sonara.paths import KEYMAP_PATH, ensure_sonara_dir

# Key/modifier tables and the default chord are platform-specific; the resolver
# pulls them from the active backend via get_platform() at call time (lazy -- no
# import-time OS dispatch). The ONLY sys.platform branch stays in platform/__init__.

# action -> the speechd protocol message it sends. The hotkey-bindable action set
# is deliberately small: Up (restart the latest turn), flush, play/pause, mute,
# session cycling and speech-rate. (stop / repeat / skip stay reachable via the
# CLI; they are just not hotkey actions.) One message, always the last: there is
# no stepping between paragraphs or older turns.
ACTION_MESSAGES = {
    "nav_start": {"type": "nav", "to": "first"},   # restart the latest turn from the top
    "flush": {"type": "flush_session"},            # flush the engaged session to the end
    "pause": {"type": "pause"},     # play/pause toggle (valid action; UNBOUND by default -- mute covers it)
    "mute": {"type": "mute"},       # global mute cycle (unmuted/muted/super muted)
    "next_session": {"type": "next_session"},   # cycle the active reader
    "faster": {"type": "set_rate", "delta": 25},
    "slower": {"type": "set_rate", "delta": -25},
}

# Action -> default key. The chord modifiers (Win+Alt) come from the active
# backend's default_mods(). Only nav_start/flush + mute + next_session are bound
# out of the box; pause and faster/slower are valid actions but ship UNBOUND
# (blank by default) so the default keymap stays minimal: users add a key in
# keymap.json if they want one.
# The keys dodge Windows 11's own Win+Alt chords (#160): Win+Alt+Up/Down snap
# windows, so restart/flush use Home (back to the start) and End (to the end);
# Win+Alt+M is Game Bar's microphone toggle and Win+Alt+P was taken too, so
# mute is S (silence) and next_session is N (next).
_DEFAULT_KEYS = {
    "nav_start": "home", "flush": "end",
    "mute": "s", "next_session": "n",   # pause unbound; mute covers it.
}


def _keytables():
    """(key_codes, mod_masks) for the active platform (lazy -- no import-time dispatch)."""
    from sonara.platform import get_platform
    hk = get_platform().hotkey
    return hk.key_codes(), hk.mod_masks()


def default_keymap() -> dict:
    """The default action->binding map for the active platform (per-OS chord)."""
    from sonara.platform import get_platform
    mods = get_platform().hotkey.default_mods()
    return {action: {"key": key, "mods": list(mods)}
            for action, key in _DEFAULT_KEYS.items()}


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
    """Resolve an action->binding map into the listener's registration list.

    Each output entry: {action, keyCode, modifiers, message}. An entry whose key
    is empty/None is treated as UNBOUND and skipped (no hotkey registered) -- this
    lets keymap.json explicitly clear an action that has a default binding. Raises
    ValueError on an unknown key name, unknown modifier name, or unknown action.
    """
    if keymap is None:
        keymap = default_keymap()
    key_codes, mod_masks = _keytables()
    resolved = []
    for action, binding in keymap.items():
        if action not in ACTION_MESSAGES:
            raise ValueError("unknown action: {0}".format(action))
        key = (binding.get("key") or "").lower()
        if not key:
            continue                    # explicitly unbound -> no hotkey
        if key not in key_codes:
            raise ValueError("unknown key: {0}".format(binding.get("key")))
        mask = 0
        for mod in binding.get("mods", []):
            m = (mod or "").lower()
            if m not in mod_masks:
                raise ValueError("unknown modifier: {0}".format(mod))
            mask |= mod_masks[m]
        resolved.append({
            "action": action,
            "keyCode": key_codes[key],
            "modifiers": mask,
            "message": json.dumps(ACTION_MESSAGES[action]),
        })
    return resolved


def load_keymap() -> dict:
    """Merge the user's KEYMAP_PATH over a copy of DEFAULT_KEYMAP.

    Missing or corrupt files yield a fresh DEFAULT_KEYMAP copy. A user entry
    fully replaces the default binding for that action. Entries for actions Sonara
    no longer defines are ignored, so a stale keymap.json (e.g. one binding an
    action that was since removed) does not break the whole keymap.
    """
    merged = _copy_keymap(default_keymap())
    try:
        with open(KEYMAP_PATH, "r", encoding="utf-8") as fh:
            user = json.load(fh)
    except (FileNotFoundError, ValueError, OSError):
        return merged
    if not isinstance(user, dict):
        return merged
    for action, binding in user.items():
        if action not in ACTION_MESSAGES:
            continue                       # drop bindings for removed/unknown actions
        if isinstance(binding, dict):
            merged[action] = {
                "key": binding.get("key"),
                "mods": list(binding.get("mods", [])),
            }
    return merged


def _read_user_keymap() -> dict:
    """The user's raw keymap.json overrides as a dict, or {} if missing/corrupt."""
    try:
        with open(KEYMAP_PATH, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except (FileNotFoundError, ValueError, OSError):
        return {}
    return data if isinstance(data, dict) else {}


def _write_user_keymap(user: dict) -> None:
    """Atomically persist the user's keymap.json overrides. Bindings for
    actions Sonara no longer defines (e.g. the removed nav_prev/nav_next) are
    dropped, so a rewrite also cleans a stale file."""
    user = {a: b for a, b in user.items() if a in ACTION_MESSAGES}
    ensure_sonara_dir()
    tmp = str(KEYMAP_PATH) + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(user, fh, indent=2)
        fh.flush()
        os.fsync(fh.fileno())
    os.replace(tmp, str(KEYMAP_PATH))


# A hotkey must hold at least one of these (E13); Shift alone still types.
_COMMAND_MODS = ("ctrl", "alt", "win")


def bind_action(action: str, key: str, mods: list) -> None:
    """Bind *action* to key+mods in the user's keymap.json (settings page, #34).
    The override fully replaces the default binding, exactly like a hand-edit."""
    if action not in ACTION_MESSAGES:
        raise ValueError(f"unknown action {action!r}")
    key = (key or "").strip().lower()
    if not key:
        raise ValueError("empty key")
    # Validate BEFORE persisting (#38): resolve_keymap raises on an unknown
    # name and that failure takes down EVERY hotkey, so a bad binding must
    # never reach keymap.json in the first place.
    key_codes, mod_masks = _keytables()
    if key not in key_codes:
        raise ValueError(f"unsupported key {key!r}")
    for m in (mods or []):
        if str(m).lower() not in mod_masks:
            raise ValueError(f"unsupported modifier {m!r}")
    # E13: a global hotkey without Ctrl, Alt or Win swallows that key in every
    # app (a bare 'm', or Shift+M, stops typing it anywhere). Refuse it.
    held = {mod_masks[str(m).lower()] for m in (mods or [])}
    if not held & {mod_masks[m] for m in _COMMAND_MODS if m in mod_masks}:
        raise ValueError("a hotkey needs Ctrl, Alt or Win: without one it "
                         "would take that key away from every app")
    user = _read_user_keymap()
    user[action] = {"key": key, "mods": [str(m).lower() for m in (mods or [])]}
    _write_user_keymap(user)


def unbind_action(action: str) -> None:
    """Persist 'no hotkey' for *action* in the user's keymap.json. If the action
    has a default binding, write an explicit unbound override ({"key": null}) so it
    overrides that default; if it has no default, just drop any user binding (the
    default is already unbound). Raises ValueError for an unknown action."""
    if action not in ACTION_MESSAGES:
        raise ValueError("unknown action: {0}".format(action))
    user = _read_user_keymap()
    if action in _DEFAULT_KEYS:
        user[action] = {"key": None, "mods": []}
    else:
        user.pop(action, None)
    _write_user_keymap(user)


# The Windows default chord dropped Shift (Ctrl+Shift+Alt -> Ctrl+Alt). Existing
# installs have a keymap.json materialized by an earlier `sonara install` with the
# legacy chord; this constant lets migrate_default_chord() spot those stale defaults.
_LEGACY_WINDOWS_MODS = ["ctrl", "shift", "alt"]
# What the legacy migration has always produced: the Ctrl+Alt chord on the
# pre-0.7.0 default keys. Frozen on purpose (#160): the Win+Alt default uses
# other keys, and Win+Alt+Up/Down belong to Windows. Existing keymaps keep
# their chord; reset_keymap() (settings page, `sonara keymap --reset`) moves
# a user to the current defaults.
_MIGRATED_MODS = ["ctrl", "alt"]
_LEGACY_DEFAULT_KEYS = {"nav_start": "up", "flush": "down",
                        "mute": "m", "next_session": "p"}


def migrate_default_chord() -> bool:
    """Upgrade a keymap.json still pinned to the legacy Ctrl+Shift+Alt default.

    Rewrites, in place, ONLY entries that exactly match a legacy default binding
    (the action's legacy key AND the legacy mods) to Ctrl+Alt, so a genuinely
    customized binding (different key or different mods) is preserved. A user who
    deliberately re-adds Shift can do so again. Idempotent and safe: a
    missing/corrupt keymap.json, or nothing to migrate, is a no-op returning False.
    Returns True iff it wrote a change."""
    user = _read_user_keymap()
    if not user:
        return False
    new_mods = list(_MIGRATED_MODS)
    changed = False
    for action, default_key in _LEGACY_DEFAULT_KEYS.items():
        entry = user.get(action)
        if not isinstance(entry, dict):
            continue
        if (entry.get("key") == default_key
                and list(entry.get("mods", [])) == _LEGACY_WINDOWS_MODS):
            entry["mods"] = list(new_mods)
            changed = True
    if changed:
        _write_user_keymap(user)
    return changed


def reset_keymap() -> None:
    """Replace the user's keymap.json with the current default bindings (#160):
    the way an existing install moves from its old Ctrl+Alt chords to Win+Alt.
    Every override, including explicit unbinds, is dropped."""
    _write_user_keymap(default_keymap())


def write_default_keymap_if_absent() -> bool:
    """Write DEFAULT_KEYMAP to KEYMAP_PATH if it does not exist. Returns True
    iff it wrote the file."""
    if os.path.exists(KEYMAP_PATH):
        return False
    ensure_sonara_dir()
    with open(KEYMAP_PATH, "w", encoding="utf-8") as fh:
        json.dump(default_keymap(), fh, indent=2)
        fh.flush()
        os.fsync(fh.fileno())
    return True

