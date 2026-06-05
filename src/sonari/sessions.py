from __future__ import annotations


class SessionManager:
    def __init__(self, background_policy: str = "earcon_only") -> None:
        self.background_policy = background_policy
        self._sessions: set[str] = set()
        self._foreground: "str | None" = None

    def set_foreground(self, session: str) -> None:
        self._sessions.add(session)
        self._foreground = session

    def foreground(self) -> "str | None":
        return self._foreground

    def is_foreground(self, session: str) -> bool:
        return self._foreground is not None and session == self._foreground

    def register(self, session: str) -> None:
        self._sessions.add(session)

    def unregister(self, session: str) -> None:
        self._sessions.discard(session)
        if self._foreground == session:
            self._foreground = None

    def should_speak(self, session: str) -> bool:
        return self.is_foreground(session)
