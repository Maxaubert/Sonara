from __future__ import annotations

import subprocess
import threading

from sonari.platform.macos.tts import MacTtsBackend  # Task 8 will replace with get_platform()
from sonari.platform.macos.earcon import MacEarconBackend  # removed in Task 8's flip
_MAC_TTS = MacTtsBackend()
_MAC_EARCON = MacEarconBackend()


def run_say(text: str, voice, rate: int):
    return _MAC_TTS.run(text, voice, rate)


def best_enhanced_voice() -> str:
    return _MAC_TTS.best_voice()


def play_earcon(path: str):
    return _MAC_EARCON.play(path)


_DEFAULT_WAIT_TIMEOUT = 120  # seconds; generous upper bound for even long TTS


class Speaker:
    def __init__(
        self,
        voice=None,
        rate=200,
        say_runner=run_say,
        earcon_player=play_earcon,
        earcons=None,
        _wait_timeout: float = _DEFAULT_WAIT_TIMEOUT,
    ) -> None:
        self._voice = voice
        self._rate = rate
        self._say_runner = say_runner
        self._earcon_player = earcon_player
        self._earcons = dict(earcons) if earcons else {}
        self._current = None
        self._current_lock = threading.Lock()
        self._earcon_procs: list = []
        self._wait_timeout = _wait_timeout

    def speak(self, text: str) -> bool:
        """Speak text, blocking. Return True iff the utterance COMPLETED
        (say exited 0). A cancelled/terminated utterance returns False so the
        caller can leave it marked unheard (sentence-granular replay)."""
        proc = self._say_runner(text, self._voice, self._rate)
        with self._current_lock:
            self._current = proc
        try:
            try:
                proc.wait(timeout=self._wait_timeout)
            except subprocess.TimeoutExpired:
                # 'say' hung past the generous deadline; kill it and move on.
                proc.terminate()
        finally:
            with self._current_lock:
                if self._current is proc:
                    self._current = None
        return getattr(proc, "returncode", None) == 0

    def cancel(self) -> None:
        with self._current_lock:
            proc = self._current
        if proc is not None:
            proc.terminate()

    def _reap_earcon_procs(self) -> None:
        """Non-blocking poll: discard entries whose process has finished."""
        self._earcon_procs = [p for p in self._earcon_procs if p.poll() is None]

    def earcon(self, kind: str) -> None:
        # Reap any finished earcon processes before launching a new one.
        self._reap_earcon_procs()
        path = self._earcons.get(kind)
        if path is None:
            return
        proc = self._earcon_player(path)
        if proc is not None and hasattr(proc, "poll"):
            self._earcon_procs.append(proc)

    def set_voice(self, v) -> None:
        self._voice = v

    def set_rate(self, r) -> None:
        self._rate = r
