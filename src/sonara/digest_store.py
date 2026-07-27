"""Durable per-session last-digest text: survives daemon restarts (#118).

Channels are in-memory, so a restart emptied every session's queue and the
manual session cycle could only reach sessions that had spoken SINCE the
restart - everyone else's last message was simply lost. This tiny store keeps
each session's most recent digest; at startup the daemon rehydrates a
caught-up (replay-only) channel from it for recently-seen sessions.

Follows sessions.py's storage discipline: opt-in store_path (tests stay
pure), best-effort atomic JSON writes (every failure swallowed), capped to
the most recent entries, missing/corrupt file tolerated.
"""
from __future__ import annotations

import json
import os

_TEXT_MAX = 4000   # bounds the file: 200 sessions x 4KB worst case


class DigestStore:
    def __init__(self, store_path=None, store_cap: int = 200) -> None:
        self._store_path = store_path
        self._store_cap = store_cap
        self._digests: "dict[str, str]" = {}
        if store_path is not None:
            self._load()

    def get(self, session: str) -> "str | None":
        return self._digests.get(session)

    def items(self) -> "list[tuple[str, str]]":
        return list(self._digests.items())

    def set(self, session: str, text) -> None:
        """Record *session*'s latest digest text. Empty/invalid input is ignored
        (a digest never goes blank; it is replaced or forgotten)."""
        if not (isinstance(session, str) and session and text):
            return
        self._digests.pop(session, None)      # re-insert -> newest position (cap keeps recent)
        self._digests[session] = str(text)[:_TEXT_MAX]
        self._persist()

    def forget(self, session: str) -> None:
        if self._digests.pop(session, None) is not None:
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
        for sid, text in data.items():
            if isinstance(sid, str) and sid and isinstance(text, str) and text:
                self._digests[sid] = text[:_TEXT_MAX]

    def _persist(self) -> None:
        if self._store_path is None:
            return
        try:
            data = dict(list(self._digests.items())[-self._store_cap:])
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
