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
        self._epoch = 0

    def speak(self, text: str, cancel_epoch=None) -> bool:
        self.spoken.append(text)
        return self.complete

    def cancel_epoch(self) -> int:
        return self._epoch

    def earcon(self, kind: str) -> None:
        self.earcons.append(kind)

    def cancel(self) -> None:
        self.cancels += 1
        self._epoch += 1

    def set_rate(self, r: int) -> None:
        self.rates.append(r)

    def set_voice(self, v) -> None:
        self.voices.append(v)


class ChannelQueueProxy:
    """Bridges old queue-based tests to the new per-session channel model.

    len(proxy)         = total pending (unspoken) items across all channels
    proxy.pop_next()   = next item from the router (advances cursor)
    proxy.enqueue(item)= append item to its session's channel (marks turn_done
                         so the item is immediately readable at minqueue==1)
    """

    def __init__(self, daemon: SpeechDaemon) -> None:
        self._daemon = daemon

    # ------------------------------------------------------------------
    # Old SpeechQueue API surface used by tests
    # ------------------------------------------------------------------

    def enqueue(self, item) -> None:
        ch = self._daemon.router.channel(item.session)
        ch.append(item)
        ch.turn_done = True          # make immediately available at minqueue==1
        # Authorize non-fg sessions to be readable (test helpers explicitly seed
        # items into sessions that may not be fg; the router must be able to
        # return them via pop_next() in those tests).
        fg = self._daemon.sessions.foreground()
        if item.session != fg:
            self._daemon.router._speakable.add(item.session)
        self._daemon._wake.set()

    def pop_next(self):
        return self._daemon.router.next_item()

    def __len__(self) -> int:
        return sum(ch.pending() for ch in self._daemon.router.channels.values())


def make_daemon(verbosity: str = "everything", foreground: "str | None" = "fg"):
    """Build a SpeechDaemon wired to a FakeSpeaker.

    Returns (daemon, queue_proxy, speaker, sessions, config).
    ``queue_proxy`` is a ChannelQueueProxy that exposes the old SpeechQueue
    API (len / pop_next / enqueue) so legacy tests continue to work without
    modification while the real SpeechQueue is gone.
    """
    speaker = FakeSpeaker()
    sessions = SessionManager()
    if foreground is not None:
        sessions.set_foreground(foreground)
    config = {k: (v.copy() if isinstance(v, dict) else v) for k, v in DEFAULTS.items()}
    config["verbosity"] = verbosity
    daemon = SpeechDaemon(speaker, sessions, config)
    queue = ChannelQueueProxy(daemon)
    return daemon, queue, speaker, sessions, config
