"""Claude Code hook delivery: Sonara's exec-form hooks in the user's
~/.claude/settings.json, generated from the plugin's hooks/hooks.json, and
their doctor row. This manages Claude Code settings, not the Windows
supervisor, which only calls it from its install, uninstall and doctor
steps (audit section 2)."""
from __future__ import annotations

import os

from sonara import paths


# ---------------------------------------------------------------------------
# exec-form hooks generated from the plugin's hooks/hooks.json
# ---------------------------------------------------------------------------

# The settings.json hooks are GENERATED from the plugin's own hooks/hooks.json,
# the single source of the event set (H4/E17): a hand-kept copy here had
# already lost PostToolUse, so CHOICE_ANSWERED never fired for settings.json
# installs. Each plugin entry ("<launcher> <Event>", run by Claude Code's
# shell) becomes exec-form, the resolved pythonw.exe baked in at install time
# by install_hooks(): command = pythonw, args = [hook, Event].

def _plugin_hooks_path(hook_py: str, plugin_root: "str | None" = None) -> str:
    """hooks/hooks.json of *plugin_root*, else of the plugin *hook_py* lives in
    (<root>/bin/sonara-hook), else of the tree this code runs from."""
    if plugin_root:
        return os.path.join(plugin_root, "hooks", "hooks.json")
    beside = os.path.join(os.path.dirname(os.path.dirname(hook_py)),
                          "hooks", "hooks.json")
    if os.path.isfile(beside):
        return beside
    return os.path.join(paths.repo_root(), "hooks", "hooks.json")


def load_plugin_hooks(hooks_json: str) -> dict:
    """The {event: [entry, ...]} map of a plugin hooks.json. Raises ValueError
    (with the path) when it is missing or malformed."""
    import json
    try:
        with open(hooks_json, "r", encoding="utf-8") as fh:
            hooks = json.load(fh).get("hooks")
    except (OSError, ValueError, AttributeError) as exc:
        raise ValueError("cannot read the plugin hooks file {0}: {1}".format(
            hooks_json, exc)) from exc
    if not isinstance(hooks, dict):
        raise ValueError("{0} has no 'hooks' object".format(hooks_json))
    return hooks


def _hook_event_arg(command: str) -> str:
    """The event argument of a plugin hook command: its last word."""
    words = str(command).split()
    return words[-1].strip("'\"") if words else ""


def _exec_form_hooks(pythonw: str, hook_py: str,
                     plugin_root: "str | None" = None) -> dict:
    """{event: [entry, ...]} in exec form, one entry per plugin entry."""
    plugin = load_plugin_hooks(_plugin_hooks_path(hook_py, plugin_root))
    out = {}
    for event, entries in plugin.items():
        converted = []
        for entry in entries:
            hooks = [{"type": "command", "command": pythonw,
                      "args": [hook_py, _hook_event_arg(h.get("command", ""))]}
                     for h in entry.get("hooks", []) if h.get("type") == "command"]
            if hooks:
                converted.append({"matcher": entry.get("matcher", ""), "hooks": hooks})
        if converted:
            out[event] = converted
    return out


def build_hooks_json(pythonw: str, hook_py: str,
                     plugin_root: "str | None" = None) -> str:
    """Return the exec-form hooks JSON ({"hooks": {...}}) for settings.json."""
    import json
    return json.dumps({"hooks": _exec_form_hooks(pythonw, hook_py, plugin_root)},
                      indent=2)


# ---------------------------------------------------------------------------
# ~/.claude/settings.json hook delivery (Windows uses exec-form hooks here,
# since the plugin's shell-form manifest cannot spawn the Python hook on win32)
# ---------------------------------------------------------------------------

def claude_settings_path() -> str:
    """Path to the user-scope Claude Code settings.json."""
    return os.path.join(os.path.expanduser("~"), ".claude", "settings.json")


def settings_has_sonara_hooks(settings_path: str) -> bool:
    """True if settings.json contains at least one Sonara hook entry (identified by
    the structured SONARA_HOOK_MARKER sentinel, #23).

    Defensive at every level: a hand-edited settings.json can have any shape
    (hooks a list, an entry a string, args not a list). 'sonara doctor' must never
    crash on it -- any unexpected shape simply yields False (M9)."""
    import json
    try:
        with open(settings_path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except Exception:
        return False
    hooks = (data or {}).get("hooks", {}) if isinstance(data, dict) else {}
    if not isinstance(hooks, dict):
        return False
    try:
        for entries in hooks.values():
            if not isinstance(entries, list):
                continue
            for entry in entries:
                if not isinstance(entry, dict):
                    continue
                inner = entry.get("hooks", [])
                if not isinstance(inner, list):
                    continue
                if any(_hook_is_sonara(h) for h in inner):
                    return True
    except Exception:  # noqa: BLE001 - doctor must never raise on malformed input
        return False
    return False


def settings_has_sonara_plugin(settings_path: str) -> bool:
    """True if the Sonara plugin is enabled in settings.json. When it is, the
    plugin's hooks/hooks.json supplies the hooks, so a hand-wired settings.json
    block is not required (and would double-fire). Tolerant of any malformed shape
    (doctor must never crash) -- M9."""
    import json
    try:
        with open(settings_path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except Exception:
        return False
    if not isinstance(data, dict):
        return False
    plugins = data.get("enabledPlugins", {})
    if not isinstance(plugins, dict):
        return False
    try:
        for name, enabled in plugins.items():
            # keys look like "sonara@sonara"; match the plugin-name part.
            if enabled and str(name).split("@", 1)[0] == "sonara":
                return True
    except Exception:  # noqa: BLE001 - doctor must never raise on malformed input
        return False
    return False


# Structured, collision-proof sentinel stamped on every hook Sonara writes.
# Identifying our own entries by this key (not by a "sonara-hook" substring scan
# over command+args) means a user's look-alike hook is never false-clobbered, and
# presence-check and removal can never diverge.
SONARA_HOOK_MARKER = "_sonara"


def _sonara_hook_paths(settings_path: str) -> list:
    """The baked script path (args[0]) of every Sonara hook in settings.json.
    Never raises (doctor must not crash); tolerant of any malformed shape."""
    import json
    try:
        with open(settings_path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except Exception:
        return []
    out = []
    hooks = (data or {}).get("hooks", {}) if isinstance(data, dict) else {}
    if not isinstance(hooks, dict):
        return out
    try:
        for entries in hooks.values():
            if not isinstance(entries, list):
                continue
            for entry in entries:
                if not isinstance(entry, dict):
                    continue
                inner = entry.get("hooks", [])
                if not isinstance(inner, list):
                    continue
                for h in inner:
                    if _hook_is_sonara(h):
                        args = h.get("args") or []
                        if isinstance(args, list) and args:
                            out.append(str(args[0]))
    except Exception:  # noqa: BLE001 - doctor must never raise on malformed input
        return out
    return out


def _build_hooks_dict(pythonw: str, hook_py: str,
                      plugin_root: "str | None" = None) -> dict:
    """Return {event: [entry, ...]} for Sonara's exec-form hooks (generated
    from the plugin's hooks/hooks.json), each hook stamped with the
    SONARA_HOOK_MARKER sentinel."""
    hooks = _exec_form_hooks(pythonw, hook_py, plugin_root)
    for entries in hooks.values():
        for entry in entries:
            for h in entry.get("hooks", []):
                h[SONARA_HOOK_MARKER] = True
    return hooks


def _hook_is_sonara(h: dict) -> bool:
    """True if a single hook dict is one Sonara wrote (structured sentinel)."""
    return isinstance(h, dict) and h.get(SONARA_HOOK_MARKER) is True


def _entry_is_sonara(entry: dict, hook_py: str = "") -> bool:
    """True if a settings.json hook entry belongs to Sonara. Keyed on the
    structured sentinel, not a free-text marker (hook_py kept for call-compat)."""
    return any(_hook_is_sonara(h) for h in entry.get("hooks", []))


def _load_settings(settings_path: str) -> dict:
    """Read settings.json tolerantly. Missing/empty -> {}. Unparseable -> ValueError
    (never clobber a file we cannot understand)."""
    import json
    if not os.path.exists(settings_path):
        return {}
    try:
        with open(settings_path, "r", encoding="utf-8") as fh:
            text = fh.read().strip()
    except OSError as exc:
        raise ValueError("cannot read {0}: {1}".format(settings_path, exc)) from exc
    if not text:
        return {}
    try:
        data = json.loads(text)
    except ValueError as exc:
        raise ValueError(
            "{0} is not valid JSON ({1}); refusing to overwrite. Fix or remove it, "
            "then re-run 'sonara install'.".format(settings_path, exc)) from exc
    return data if isinstance(data, dict) else {}


def _write_settings(settings_path: str, data: dict) -> None:
    """Atomically replace settings.json. This is the user's SHARED Claude config,
    so a truncating in-place write that fails mid-serialization would corrupt the
    whole file. Write a temp file in the same dir, fsync, then os.replace (atomic
    on POSIX + Windows). Mirrors the temp+replace pattern in keymap.py."""
    import json
    import tempfile
    parent = os.path.dirname(settings_path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    fd, tmp = tempfile.mkstemp(
        dir=parent or ".", prefix=".sonara-settings-", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(data, fh, indent=2)
            fh.write("\n")
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, settings_path)
    except BaseException:
        try:
            os.remove(tmp)
        except OSError:
            pass
        raise


def remove_hooks_from_settings(settings_path: str, hook_py: str, _data=None) -> None:
    """Remove only Sonara's hook entries; prune emptied events / hooks. When *_data*
    is given, prune it in place and DO NOT write (used by merge)."""
    data = _data if _data is not None else _load_settings(settings_path)
    hooks = data.get("hooks", {})
    for event in list(hooks.keys()):
        hooks[event] = [e for e in hooks[event] if not _entry_is_sonara(e, hook_py)]
        if not hooks[event]:
            del hooks[event]
    if not hooks and "hooks" in data:
        del data["hooks"]
    if _data is None:
        _write_settings(settings_path, data)


def _validate_hooks_shape(data: dict, settings_path: str) -> None:
    """Raise a friendly ValueError if settings.json has a 'hooks' value we cannot
    safely merge into (must be absent or a dict of event -> list). Without this a
    malformed shape raises a cryptic AttributeError mid-merge."""
    hooks = data.get("hooks")
    if hooks is None:
        return
    if not isinstance(hooks, dict):
        raise ValueError(
            "{0} has a 'hooks' value that is not an object; refusing to modify it. "
            "Fix or remove it, then re-run 'sonara install'.".format(settings_path))
    for event, entries in hooks.items():
        if not isinstance(entries, list):
            raise ValueError(
                "{0} hooks['{1}'] is not a list; refusing to modify it. Fix or "
                "remove it, then re-run 'sonara install'.".format(
                    settings_path, event))


def merge_hooks_into_settings(settings_path: str, pythonw: str, hook_py: str,
                              plugin_root: "str | None" = None) -> None:
    """Idempotently add Sonara's exec-form hooks to settings.json: drop any prior
    Sonara entries (self-heal across path changes), then append the current ones.
    Preserves all other keys and all non-Sonara hook entries."""
    data = _load_settings(settings_path)
    _validate_hooks_shape(data, settings_path)
    new_hooks = _build_hooks_dict(pythonw, hook_py, plugin_root)  # before any change
    remove_hooks_from_settings(settings_path, hook_py, _data=data)  # in-place prune
    hooks = data.setdefault("hooks", {})
    for event, entries in new_hooks.items():
        hooks.setdefault(event, []).extend(entries)
    _write_settings(settings_path, data)


def _hook_py(plugin_root: "str | None" = None) -> str:
    """Absolute path to the plugin's bin/sonara-hook (pure-Python hook entry):
    under *plugin_root* when given, else under the tree this code runs from."""
    return os.path.join(plugin_root or paths.repo_root(), "bin", "sonara-hook")


# ---------------------------------------------------------------------------
# install / uninstall / doctor steps (called by the platform supervisor)
# ---------------------------------------------------------------------------

def install_hooks(pythonw: str, plugin_root: "str | None" = None) -> None:
    """Write Sonara's exec-form hooks, baked to *pythonw*, into settings.json.

    But ONLY when the sonara plugin is NOT enabled: an enabled plugin already
    supplies these exact hooks via its hooks/hooks.json, so also writing them
    to settings.json fires every event TWICE -- each assistant message is then
    spoken twice (#44). When the plugin is on, heal any hooks a prior install
    left behind and write nothing new. Raises ValueError on an unparseable
    settings.json (never clobbered)."""
    settings = claude_settings_path()
    if settings_has_sonara_plugin(settings):
        if settings_has_sonara_hooks(settings):
            remove_hooks_from_settings(settings, _hook_py())
            print("Removed duplicate Sonara hooks from {0}; the enabled sonara "
                  "plugin already supplies them.".format(settings))
        else:
            print("Sonara plugin enabled; hooks come from the plugin "
                  "(nothing written to {0}).".format(settings))
    else:
        merge_hooks_into_settings(settings, pythonw, _hook_py(plugin_root),
                                  plugin_root=plugin_root)
        print("Wrote Sonara hooks to: {0}".format(settings))


def uninstall_hooks() -> None:
    """Remove Sonara's hooks from settings.json. An unparseable file is left
    alone and the user is told (E19), so this never raises ValueError."""
    try:
        remove_hooks_from_settings(claude_settings_path(), _hook_py())
        print("Removed Sonara hooks from: {0}".format(claude_settings_path()))
    except ValueError as exc:
        # E19: an unparseable settings.json is never rewritten; finish the
        # rest of the uninstall instead of stopping half-way on a traceback.
        print("Could not remove Sonara hooks from settings.json: {0} "
              "Remove the Sonara entries by hand.".format(exc))


def doctor_row() -> tuple:
    """Sonara hooks come from EITHER a hand-wired settings.json block
    (written by 'sonara install') OR the enabled 'sonara' plugin (its
    hooks/hooks.json). For the settings.json path, go RED if a hook is present
    but its baked script path no longer exists -- the stale-after-plugin-update
    case that otherwise stops speech silently while doctor stayed green (#8).
    Also go RED when BOTH sources are present: each event then fires twice and
    every message is spoken twice (#44); 're-run sonara install' heals it."""
    path = claude_settings_path()
    if settings_has_sonara_hooks(path) and settings_has_sonara_plugin(path):
        return ("hooks installed", False,
                "hooks registered TWICE (settings.json + the sonara plugin) -- "
                "every message is spoken twice; re-run 'sonara install' to heal "
                "(it drops the settings.json copy when the plugin is enabled)")
    if settings_has_sonara_hooks(path):
        missing = [p for p in _sonara_hook_paths(path)
                   if p and not os.path.exists(p)]
        if missing:
            return ("hooks installed", False,
                    "hook script missing: {0} (stale after a plugin update; "
                    "re-run 'sonara install')".format(missing[0]))
        return ("hooks installed", True, "{0} (settings.json)".format(path))
    if settings_has_sonara_plugin(path):
        return ("hooks installed", True, "via the sonara plugin")
    return ("hooks installed", False,
            "no Sonara hooks in {0} and the sonara plugin is not enabled "
            "(run 'sonara install', or enable the sonara plugin)".format(path))
