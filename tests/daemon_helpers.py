from sonari.queue import SpeechQueue
from sonari.sessions import SessionManager
from sonari.daemon import SpeechDaemon
from sonari.config import DEFAULTS


class FakeSpeaker:
    """Records every Speaker call instead of touching audio."""

    def __init__(self):
        self.spoken: list[str] = []
        self.earcons: list[str] = []
        self.cancels: int = 0
        self.rates: list[int] = []
        self.voices: list = []
        self.complete = True          # next speak() reports completed?

    def speak(self, text: str) -> bool:
        self.spoken.append(text)
        return self.complete

    def earcon(self, kind: str) -> None:
        self.earcons.append(kind)

    def cancel(self) -> None:
        self.cancels += 1

    def set_rate(self, r: int) -> None:
        self.rates.append(r)

    def set_voice(self, v) -> None:
        self.voices.append(v)


def make_daemon(verbosity: str = "everything", foreground: "str | None" = "fg"):
    """Build a SpeechDaemon wired to a real SpeechQueue + FakeSpeaker."""
    queue = SpeechQueue()
    speaker = FakeSpeaker()
    sessions = SessionManager()
    if foreground is not None:
        sessions.set_foreground(foreground)
    config = {k: (v.copy() if isinstance(v, dict) else v) for k, v in DEFAULTS.items()}
    config["verbosity"] = verbosity
    daemon = SpeechDaemon(queue, speaker, sessions, config)
    return daemon, queue, speaker, sessions, config
