from __future__ import annotations


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
    def __init__(self, background_policy: str = "earcon_only") -> None:
        self.background_policy = background_policy
        # session id -> cwd basename (or None). Insertion-ordered (dict) so a future
        # list/cycle is stable; membership/`in`/len behave like the old set.
        self._sessions: "dict[str, str | None]" = {}
        self._foreground: "str | None" = None
        self._pinned: "str | None" = None      # None = auto (follow last prompt)

    def _record(self, session: str, cwd) -> None:
        folder = _basename(cwd)
        if session not in self._sessions:
            self._sessions[session] = folder
        elif folder:                            # update only with a non-empty name
            self._sessions[session] = folder

    def set_foreground(self, session: str, cwd=None) -> None:
        self._record(session, cwd)
        self._foreground = session

    def foreground(self) -> "str | None":
        """The session that owns the voice: the pinned one if pinned, else the last
        session to submit a prompt / start."""
        return self._pinned if self._pinned is not None else self._foreground

    def is_foreground(self, session: str) -> bool:
        fg = self.foreground()
        return fg is not None and session == fg

    def register(self, session: str, cwd=None) -> None:
        self._record(session, cwd)

    def unregister(self, session: str) -> None:
        self._sessions.pop(session, None)
        if self._foreground == session:
            self._foreground = None
        if self._pinned == session:             # pinned session ended -> auto
            self._pinned = None

    def should_speak(self, session: str) -> bool:
        """Whether the router may serve this session in the oldest-waiting slot.

        earcon_only (default): only the foreground session gets voice time.
        any other policy: all sessions with ready content are eligible."""
        if self.background_policy == "earcon_only":
            return self.is_foreground(session)
        return True

    def pinned(self) -> "str | None":
        return self._pinned

    def folder(self, session: str) -> "str | None":
        return self._sessions.get(session)

    def pin_toggle(self) -> "tuple[str, str | None]":
        """Toggle the pin against the RAW last-prompt foreground.

        - no foreground          -> ("none", None), no change
        - already pinned to it   -> unpin -> ("unpinned", folder)
        - otherwise              -> pin it -> ("pinned", folder)
        """
        cur = self._foreground
        if cur is None:
            return ("none", None)
        if self._pinned == cur:
            self._pinned = None
            return ("unpinned", self._sessions.get(cur))
        self._pinned = cur
        return ("pinned", self._sessions.get(cur))
