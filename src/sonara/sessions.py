from __future__ import annotations

import json
import os
import time


def _basename(cwd) -> "str | None":
    """Portable last path component of *cwd*, handling both / and \\ separators
    regardless of host OS (a Windows cwd is named correctly even on a macOS runner).
    Empty/None -> None."""
    if not cwd:
        return None
    s = str(cwd).replace("\\", "/").rstrip("/")
    base = s.rsplit("/", 1)[-1]
    return base or None


class SessionManager:
    def __init__(self, background_policy: str = "earcon_only",
                 store_path=None, store_cap: int = 200,
                 seen_path=None, seen_throttle_s: float = 60.0) -> None:
        self.background_policy = background_policy
        # session id -> cwd basename (or None). Insertion-ordered (dict) so a future
        # list/cycle is stable; membership/`in`/len behave like the old set.
        self._sessions: "dict[str, str | None]" = {}
        self._foreground: "str | None" = None
        # Optional durable folder map. cwd only arrives on SessionStart /
        # UserPromptSubmit hooks, so a daemon restart would otherwise lose the folder
        # name of any background session that isn't re-prompted -> the session-change
        # announcement falls back to "another session". When store_path is set we
        # persist the name so it survives restarts. Opt-in: tests pass no path and
        # stay pure (no I/O).
        self._store_path = store_path
        self._store_cap = store_cap
        # Wall-clock last activity, persisted to its own tiny store: deploys
        # and hook respawns restart the daemon constantly, and a memory-only
        # timestamp made a session that spoke five minutes ago rank as
        # inactive after every restart (the mute-persistence lesson, #65).
        # Writes are throttled to seen_throttle_s per session; minute-level
        # resolution is plenty for the Sessions tab's hour threshold.
        self._last_seen: "dict[str, float]" = {}
        self._seen_path = seen_path
        self._seen_throttle_s = seen_throttle_s
        self._persisted_seen: "dict[str, float]" = {}
        if seen_path is not None:
            self._load_seen()
        if store_path is not None:
            self._load()

    def touch(self, session: str) -> None:
        """Record hook traffic from *session* now (ranks the Sessions tab)."""
        if not (isinstance(session, str) and session):
            return
        now = time.time()
        self._last_seen[session] = now
        if self._seen_path is None:
            return
        last = self._persisted_seen.get(session)
        if last is None or now - last >= self._seen_throttle_s:
            self._persist_seen()

    def last_seen(self, session: str) -> "float | None":
        """Epoch seconds of the last hook traffic this run, or None if none."""
        return self._last_seen.get(session)

    def _record(self, session: str, cwd) -> None:
        self.touch(session)
        folder = _basename(cwd)
        changed = False
        if session not in self._sessions:
            self._sessions[session] = folder
            changed = folder is not None        # only a real name is worth persisting
        elif folder and self._sessions[session] != folder:
            self._sessions[session] = folder     # update only with a non-empty name
            changed = True
        if changed:
            self._persist()

    def set_foreground(self, session: str, cwd=None) -> None:
        self._record(session, cwd)
        self._foreground = session

    def foreground(self) -> "str | None":
        """The session that owns the voice: the last session to submit a prompt / start."""
        return self._foreground

    def is_foreground(self, session: str) -> bool:
        fg = self.foreground()
        return fg is not None and session == fg

    def register(self, session: str, cwd=None) -> None:
        self._record(session, cwd)

    def unregister(self, session: str) -> None:
        existed = session in self._sessions
        self._sessions.pop(session, None)
        seen = self._last_seen.pop(session, None)
        if seen is not None and self._seen_path is not None:
            self._persist_seen()                 # removal is durable too
        if self._foreground == session:
            self._foreground = None
        if existed:
            self._persist()                      # don't resurrect an ended session

    def should_speak(self, session: str) -> bool:
        """Whether the router may serve this session in the oldest-waiting slot.

        earcon_only (default): only the foreground session gets voice time.
        any other policy: all sessions with ready content are eligible."""
        if self.background_policy == "earcon_only":
            return self.is_foreground(session)
        return True

    def folder(self, session: str) -> "str | None":
        return self._sessions.get(session)

    def ids(self) -> "list[str]":
        """Known session ids, insertion-ordered (live plus persisted)."""
        return list(self._sessions)

    # --- durable last-seen map (opt-in via seen_path) ---------------------

    def _load_seen(self) -> None:
        """Seed last-seen from the store. Missing/corrupt file is a silent
        no-op; junk entries are skipped."""
        try:
            with open(str(self._seen_path), "r", encoding="utf-8") as fh:
                data = json.load(fh)
        except (FileNotFoundError, ValueError, OSError):
            return
        if not isinstance(data, dict):
            return
        for sid, seen in data.items():
            if isinstance(sid, str) and isinstance(seen, (int, float)):
                self._last_seen[sid] = float(seen)
                self._persisted_seen[sid] = float(seen)

    def _persist_seen(self) -> None:
        """Best-effort atomic write of the last-seen map, capped to the most
        recent store_cap by timestamp. Failures are swallowed."""
        if self._seen_path is None:
            return
        try:
            data = dict(sorted(self._last_seen.items(),
                               key=lambda kv: kv[1])[-self._store_cap:])
            path = str(self._seen_path)
            parent = os.path.dirname(path)
            if parent:
                os.makedirs(parent, exist_ok=True)
            tmp = path + ".tmp"
            with open(tmp, "w", encoding="utf-8") as fh:
                json.dump(data, fh)
                fh.flush()
                os.fsync(fh.fileno())
            os.replace(tmp, path)
            self._persisted_seen = dict(data)
        except OSError:
            pass

    # --- durable folder map (opt-in via store_path) -----------------------

    def _load(self) -> None:
        """Seed the folder map from the store. Only entries with a real folder name
        are taken, and never over an already-known live name. Missing/corrupt file
        is a silent no-op."""
        try:
            with open(str(self._store_path), "r", encoding="utf-8") as fh:
                data = json.load(fh)
        except (FileNotFoundError, ValueError, OSError):
            return
        if not isinstance(data, dict):
            return
        for sid, folder in data.items():
            if isinstance(sid, str) and folder and sid not in self._sessions:
                self._sessions[sid] = folder

    def _persist(self) -> None:
        """Best-effort atomic write of the known folder names, capped to the most
        recent store_cap. Persistence must never break session handling, so every
        failure is swallowed."""
        if self._store_path is None:
            return
        try:
            data = {k: v for k, v in self._sessions.items() if v}
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
