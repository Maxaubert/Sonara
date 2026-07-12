from __future__ import annotations

import subprocess
import threading

_DEFAULT_WAIT_TIMEOUT = 120  # seconds; generous upper bound for even long TTS


class Speaker:
    def __init__(
        self,
        voice=None,
        rate=200,
        say_runner=None,
        earcon_player=None,
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
        self._cancel_epoch = 0          # bumped by cancel(); closes the synth-gap race
        self._earcon_procs: list = []
        self._wait_timeout = _wait_timeout

    def cancel_epoch(self) -> int:
        """The current cancel epoch. The daemon captures this at CLAIM time (under
        its own lock) and passes it to speak(), so a cancel landing in the gap
        between claim and speak() is detected (M2)."""
        with self._current_lock:
            return self._cancel_epoch

    def speak(self, text: str, cancel_epoch=None, on_play=None) -> bool:
        """Speak text, blocking. Return True iff the utterance COMPLETED
        (say exited 0). A cancelled/terminated utterance returns False so the
        caller can leave it marked unheard (sentence-granular replay).

        *cancel_epoch* is the baseline to compare against. The daemon captures it
        at the moment it claims the item (under its lock) and passes it here; a
        cancel() arriving between the claim and this call bumps the live epoch past
        the captured baseline, so we still detect it. When None, the baseline is the
        epoch read here (the prior single-call behavior).

        *on_play*, when given, is forwarded to the say_runner, which fires it at
        PLAYBACK start (after synthesis). The daemon passes its audio-duck routine
        here so other apps' audio dips when the voice starts, not seconds earlier
        while a slow neural voice is still synthesizing. Omitted -> the classic
        three-arg say_runner call, so existing runners keep working."""
        if self._say_runner is None:
            return False
        # Establish the baseline epoch BEFORE synthesis. say_runner (TTS synthesis)
        # can take tens-hundreds of ms, during which there is no proc to cancel —
        # a cancel() arriving in that window used to be a silent no-op and the
        # utterance played anyway. If the epoch advanced past the baseline while we
        # synthesized, a cancel landed: honor it by terminating immediately and
        # reporting the utterance as NOT completed (so the caller replays it).
        with self._current_lock:
            epoch = self._cancel_epoch if cancel_epoch is None else cancel_epoch
        if on_play is None:
            proc = self._say_runner(text, self._voice, self._rate)
        else:
            proc = self._say_runner(text, self._voice, self._rate, on_play)
        with self._current_lock:
            interrupted = self._cancel_epoch != epoch
            if not interrupted:
                self._current = proc
        if interrupted:
            proc.terminate()
            return False
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
            self._cancel_epoch += 1     # so a speak() mid-synthesis aborts on return
            proc = self._current
        if proc is not None:
            proc.terminate()

    def _reap_earcon_procs(self) -> None:
        """Non-blocking poll: discard entries whose process has finished."""
        self._earcon_procs = [p for p in self._earcon_procs if p.poll() is None]

    def earcon(self, kind: str) -> None:
        if self._earcon_player is None:
            return
        # Reap any finished earcon processes before launching a new one.
        self._reap_earcon_procs()
        path = self._earcons.get(kind)
        if path is None:
            return
        proc = self._earcon_player(path)
        if proc is not None and hasattr(proc, "poll"):
            self._earcon_procs.append(proc)

    def earcon_pids(self) -> "list[int]":
        """PIDs of the live earcon helper subprocesses (so the ducker excludes
        Sonara's own beeps). Test seam: a `_earcon_pids` attribute overrides."""
        injected = getattr(self, "_earcon_pids", None)
        if injected is not None:
            return list(injected)
        return [p.pid for p in self._earcon_procs if p.poll() is None]

    def set_voice(self, v) -> None:
        self._voice = v

    def set_rate(self, r) -> None:
        self._rate = r
