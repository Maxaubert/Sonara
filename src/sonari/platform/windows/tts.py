"""Windows OneCore TTS backend via PyWinRT — synthesize + winsound playback.

OneCore (Windows.Media.SpeechSynthesis) synthesizes a WAV stream; we play it
with stdlib ``winsound`` from a temp file. The earlier MediaPlayer-based
playback crashed the process with a native access violation after ~80 utterances
(a PyWinRT MediaPlayer fragility — synthesis is fine, playback is not), which is
the daemon-death bug. ``winsound`` is COM-free, in-process, and stress-survives.

To fit Sonari's say_runner contract (the Speaker orchestrates a proc-like
object), run() returns a _TtsHandle whose .wait(timeout)/.terminate()/
.returncode mimic subprocess.Popen.

WINDOWS-only: every winrt.* / winsound import is LAZY (inside methods) so this
module imports cleanly on macOS/Linux for the mock test suite. "Working" under
the mocks is NOT a claim that real OneCore playback works — only Windows is.

Requirements (Windows only):
    pip install winrt-runtime winrt-Windows.Media.SpeechSynthesis \
                winrt-Windows.Storage.Streams

NOTE: winsound is a single output channel for speech. Earcons are played in a
separate windowless helper process (see earcon.py) so their audio session mixes
with speech (shared-mode) rather than cutting it.
"""
from __future__ import annotations

import io
import os
import subprocess
import tempfile
import threading
import wave
from typing import Optional

from sonari.platform.base import TtsBackend

_BASELINE_WPM: float = 200.0  # Sonari's default wpm maps to SpeakingRate 1.0

_WINRT_INSTALL_HINT = (
    "PyWinRT is not installed, so Sonari cannot synthesize speech. Install it: "
    "pip install winrt-runtime winrt-Windows.Media.SpeechSynthesis "
    "winrt-Windows.Storage.Streams"
)


def _winrt_available() -> bool:
    """True if the OneCore TTS WinRT projection can be imported. Used by run()
    (actionable error) and by `sonari doctor` (so an undeclared/missing PyWinRT
    surfaces as RED, not silent no-speech behind a green doctor). (#7)"""
    try:
        import winrt.windows.media.speechsynthesis  # noqa: F401
        return True
    except Exception:
        return False


def _require_winrt() -> None:
    if not _winrt_available():
        raise RuntimeError(_WINRT_INSTALL_HINT)


def wpm_to_speaking_rate(wpm: float) -> float:
    """Map Sonari [100-400] wpm to a SpeakingRate multiplier [0.5-6.0].

    SpeakingRate is a multiplier, not an absolute wpm; values outside
    [0.5, 6.0] raise on real WinRT, so we always clamp.
    """
    return max(0.5, min(6.0, wpm / _BASELINE_WPM))


_TMP_PREFIX = "sonari-tts-"


def _sweep_stale_wavs(max_age_s: float = 300.0) -> None:
    """Best-effort cleanup of temp WAVs leaked by a prior crashed/killed daemon.
    Only removes files older than *max_age_s*, so a clip that another instance
    may still be playing is never deleted, and only our own sonari-tts-* prefix
    is touched. Never raises. (#26)"""
    import glob
    import time
    try:
        now = time.time()
        pattern = os.path.join(tempfile.gettempdir(), _TMP_PREFIX + "*.wav")
        for p in glob.glob(pattern):
            try:
                if now - os.path.getmtime(p) > max_age_s:
                    os.unlink(p)
            except OSError:
                pass
    except Exception:
        pass


def _wav_duration(data: bytes) -> float:
    """Seconds of audio in a WAV byte string (for the completion timer)."""
    try:
        with wave.open(io.BytesIO(data)) as w:
            frames = w.getnframes()
            rate = w.getframerate() or 1
            return frames / float(rate)
    except Exception:
        return 4.0   # safe fallback so wait() can't block forever


class _TtsHandle:
    """Subprocess-like handle for an in-flight winsound utterance.

    returncode: None while playing, 0 = completed normally, 1 = interrupted.
    Playback is async (winsound SND_ASYNC); a timer marks completion after the
    clip's duration. terminate() purges playback. The temp WAV is removed when
    playback ends (completion or terminate).
    """

    def __init__(self, wav_path: str, duration: float):
        import winsound
        self._winsound = winsound
        self._path = wav_path
        self._done = threading.Event()
        self.returncode: Optional[int] = None
        # +0.25s guard so the temp file isn't unlinked while still being read.
        self._timer = threading.Timer(duration + 0.25, self._complete)
        self._timer.daemon = True
        self._timer.start()

    def _cleanup(self) -> None:
        try:
            os.unlink(self._path)
        except OSError:
            pass

    def _complete(self) -> None:
        if self.returncode is None:
            self.returncode = 0
        self._cleanup()
        self._done.set()

    def wait(self, timeout: Optional[float] = None) -> int:
        completed = self._done.wait(timeout=timeout)
        if not completed:
            raise subprocess.TimeoutExpired(cmd="onecore-tts", timeout=timeout)
        return self.returncode

    def terminate(self) -> None:
        if self.returncode is None:
            self.returncode = 1
        try:
            # PlaySound(None, 0) is the documented way to stop playback on modern
            # Windows; SND_PURGE is documented as not supported there. (#17)
            self._winsound.PlaySound(None, 0)
        except Exception:
            pass
        try:
            self._timer.cancel()
        except Exception:
            pass
        self._cleanup()
        self._done.set()

    def poll(self) -> Optional[int]:
        return self.returncode


def _play_wav_bytes(data: bytes):
    """Write WAV *data* to a temp file and start async winsound playback, returning
    a _TtsHandle. Shared by the WinRT and Kokoro synth paths. If PlaySound raises
    before the handle owns the file, unlink it so a failed utterance doesn't leak a
    temp WAV (the #26 init-sweep would otherwise only reclaim it on the next start)."""
    import winsound
    fd, path = tempfile.mkstemp(suffix=".wav", prefix=_TMP_PREFIX)
    try:
        os.write(fd, data)
    finally:
        os.close(fd)
    duration = _wav_duration(data)
    try:
        winsound.PlaySound(path, winsound.SND_FILENAME | winsound.SND_ASYNC)
    except Exception:
        try:
            os.unlink(path)
        except OSError:
            pass
        raise
    return _TtsHandle(path, duration)


class WinTtsBackend(TtsBackend):
    """OneCore TTS via PyWinRT synthesis + winsound playback, with optional Kokoro
    neural voices routed through the same winsound path.

    The SpeechSynthesizer is created ONCE and reused (synthesis is stable). All
    winrt.*/winsound imports are lazy (inside methods)."""

    def __init__(self) -> None:
        self._synth = None         # reused SpeechSynthesizer (lazy)
        self._kokoro = None        # lazy KokoroEngine (only if a Kokoro voice is used)
        _sweep_stale_wavs()        # clear temp WAVs leaked by a prior crash (#26)

    def _get_kokoro(self):
        """Lazy KokoroEngine — downloads the ~316 MB model on first Kokoro voice."""
        if self._kokoro is None:
            from sonari import kokoro, paths
            self._kokoro = kokoro.KokoroEngine(paths.SONARI_DIR / "kokoro")
        return self._kokoro

    def _get_synth(self):
        if self._synth is None:
            from winrt.windows.media.speechsynthesis import (
                SpeechSynthesizer, SpeechAppendedSilence, SpeechPunctuationSilence,
            )
            s = SpeechSynthesizer()
            opts = s.options
            opts.appended_silence = SpeechAppendedSilence.MIN
            opts.punctuation_silence = SpeechPunctuationSilence.MIN
            self._synth = s
        return self._synth

    def _all_voice_infos(self) -> list:
        """Internal: all installed VoiceInformation OBJECTS (may be empty)."""
        from winrt.windows.media.speechsynthesis import SpeechSynthesizer
        return list(SpeechSynthesizer.all_voices)

    def list_voices(self) -> list:
        """ABC contract: list of selectable voice NAMES (str) — the installed
        OneCore voices PLUS the 28 Kokoro neural voices, but only when the optional
        [kokoro] extra is installed (else advertising them would let a user pick a
        voice whose first speak silently fails). Internal callers that need the WinRT
        objects use _all_voice_infos()/_best_voice_info() instead. (#16)"""
        from sonari import kokoro
        try:
            native = [v.display_name for v in self._all_voice_infos()]
        except Exception:  # noqa: BLE001 - listing must work even with no winrt
            native = []
        kokoro_voices = list(kokoro.VOICES) if kokoro.is_installed() else []
        return native + kokoro_voices

    def _best_voice_info(self, lang_prefix: str = "en-US"):
        """Select a VoiceInformation in priority order:
          1. en-US OneCore (Id path contains 'Speech_OneCore'); 2. any en-US;
          3. default_voice. Raises RuntimeError if no voices are installed.

        Internal: returns the WinRT object. The public ABC best_voice() returns
        its display NAME (str).
        """
        from winrt.windows.media.speechsynthesis import SpeechSynthesizer
        voices = self._all_voice_infos()
        if not voices:
            raise RuntimeError(
                "No TTS voices installed. Add a Speech language pack in "
                "Settings -> Time & language -> Speech -> Add voices."
            )
        ll = lang_prefix.lower()

        def _is_onecore(v) -> bool:
            return "speech_onecore" in (v.id or "").lower()

        for v in voices:
            if v.language.lower().startswith(ll) and _is_onecore(v):
                return v
        for v in voices:
            if v.language.lower().startswith(ll):
                return v
        return SpeechSynthesizer.default_voice

    def best_voice(self, lang_prefix: str = "en-US") -> str:
        """ABC contract: return the best installed voice's display NAME (str)."""
        return self._best_voice_info(lang_prefix).display_name

    def _resolve_voice(self, name):
        """Resolve a Sonari config voice-NAME (or None) to a VoiceInformation.

        Speaker passes the configured voice as a display-name string or None, but
        synth.voice requires a VoiceInformation object. Match by display_name
        (case-insensitive); fall back to best_voice() if unknown/None.
        """
        if name:
            for v in self._all_voice_infos():
                if (v.display_name or "").lower() == str(name).lower():
                    return v
        return self._best_voice_info()

    def _synthesize_wav(self, text: str, voice, rate: int) -> bytes:
        """Synthesize *text* to WAV bytes (no playback)."""
        from winrt.windows.storage.streams import DataReader

        speaking_rate = wpm_to_speaking_rate(rate)
        resolved_voice = self._resolve_voice(voice)   # raises if no voices
        synth = self._get_synth()
        synth.voice = resolved_voice
        opts = synth.options

        use_ssml = False
        try:
            opts.speaking_rate = float(speaking_rate)
        except AttributeError:
            # Win10 < 1709: speaking_rate unavailable; fall back to SSML.
            pct = int(speaking_rate * 100)
            safe = (text.replace("&", "&amp;").replace("<", "&lt;")
                    .replace(">", "&gt;"))
            text = ('<speak version="1.0" '
                    'xmlns="http://www.w3.org/2001/10/synthesis" xml:lang="en-US">'
                    '<prosody rate="{0}%">{1}</prosody></speak>'.format(pct, safe))
            use_ssml = True

        if use_ssml:
            stream = synth.synthesize_ssml_to_stream_async(text).get()
        else:
            stream = synth.synthesize_text_to_stream_async(text).get()

        size = stream.size
        reader = DataReader(stream.get_input_stream_at(0))
        reader.load_async(size).get()
        buf = bytearray(size)
        reader.read_bytes(buf)
        return bytes(buf)

    def run(self, text: str, voice, rate: int):
        """Synthesize *text* and start async winsound playback, returning a
        _TtsHandle the caller can .wait()/.terminate()/.poll().

        A Kokoro voice (af_heart, af_nicole, ...) is synthesized by the Kokoro
        engine; anything else by the native WinRT/OneCore engine. Both paths produce
        WAV bytes played through the same winsound handle (so cancel/interrupt,
        earcon mixing, and cleanup are identical)."""
        from sonari import kokoro
        if kokoro.is_kokoro_voice(voice):
            kokoro.require_installed()   # actionable error instead of a raw ImportError
            data = self._get_kokoro().wav_bytes(
                text, voice, kokoro.rate_to_speed(rate))
        else:
            _require_winrt()   # actionable error instead of a raw ImportError (#7)
            data = self._synthesize_wav(text, voice, rate)
        return _play_wav_bytes(data)
