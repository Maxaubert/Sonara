"""Per-session user preferences: display name, mute, voice override.

A tiny durable map keyed by Claude Code session id. Follows sessions.py's
storage discipline: opt-in store_path (tests stay pure), best-effort atomic
JSON writes (every failure swallowed), capped to the most recent entries,
missing/corrupt file tolerated.
"""
from __future__ import annotations

import json
import os

_ALLOWED_KEYS = ("name", "muted", "voice")
_NAME_MAX = 60


class SessionPrefs:
    def __init__(self, store_path=None, store_cap: int = 200) -> None:
        self._store_path = store_path
        self._store_cap = store_cap
        self._prefs: "dict[str, dict]" = {}
        if store_path is not None:
            self._load()

    def get(self, session: str) -> dict:
        return dict(self._prefs.get(session) or {})

    def name(self, session: str) -> "str | None":
        v = (self._prefs.get(session) or {}).get("name")
        return str(v) if v else None

    def muted(self, session: str) -> bool:
        return bool((self._prefs.get(session) or {}).get("muted"))

    def voice(self, session: str) -> "str | None":
        v = (self._prefs.get(session) or {}).get("voice")
        return str(v) if v else None

    def set(self, session: str, key: str, value) -> bool:
        """Set one pref; returns False for an unknown key or bad session id.
        A falsy name/voice clears the key; muted coerces to bool."""
        if not isinstance(session, str) or not session or key not in _ALLOWED_KEYS:
            return False
        entry = self._prefs.setdefault(session, {})
        if key == "muted":
            entry["muted"] = bool(value)
            if not entry["muted"]:
                entry.pop("muted", None)          # default is unmuted: no litter
        elif value:
            entry[key] = str(value)[:_NAME_MAX] if key == "name" else str(value)
        else:
            entry.pop(key, None)
        if not entry:
            self._prefs.pop(session, None)
        self._persist()
        return True

    def forget(self, session: str) -> None:
        if self._prefs.pop(session, None) is not None:
            self._persist()

    # --- durable store (opt-in via store_path), mirrors sessions.py -------

    def _load(self) -> None:
        try:
            with open(str(self._store_path), "r", encoding="utf-8") as fh:
                data = json.load(fh)
        except (FileNotFoundError, ValueError, OSError):
            return
        if not isinstance(data, dict):
            return
        for sid, entry in data.items():
            if isinstance(sid, str) and isinstance(entry, dict) and entry:
                kept = {k: entry[k] for k in _ALLOWED_KEYS if k in entry}
                if kept:
                    self._prefs[sid] = kept

    def _persist(self) -> None:
        if self._store_path is None:
            return
        try:
            data = {k: v for k, v in self._prefs.items() if v}
            if len(data) > self._store_cap:
                data = dict(list(data.items())[-self._store_cap:])
            path = str(self._store_path)
            parent = os.path.dirname(path)
            if parent:
                os.makedirs(parent, exist_ok=True)
            tmp = path + ".tmp"
            with open(tmp, "w", encoding="utf-8") as fh:
                json.dump(data, fh)
                fh.flush()
                os.fsync(fh.fileno())
            os.replace(tmp, path)
        except OSError:
            pass
