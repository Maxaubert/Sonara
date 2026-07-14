"""Windows OneCore TTS backend via PyWinRT -- synthesize + winsound playback.

OneCore (Windows.Media.SpeechSynthesis) synthesizes a WAV stream; we play it
with stdlib ``winsound`` from a temp file. The earlier MediaPlayer-based
playback crashed the process with a native access violation after ~80 utterances
(a PyWinRT MediaPlayer fragility -- synthesis is fine, playback is not), which is
the daemon-death bug. ``winsound`` is COM-free, in-process, and stress-survives.

To fit Sonara's say_runner contract (the Speaker orchestrates a proc-like
object), run() returns a _TtsHandle whose .wait(timeout)/.terminate()/
.returncode mimic subprocess.Popen.

WINDOWS-only: every winrt.* / winsound import is LAZY (inside methods) so this
module imports cleanly on macOS/Linux for the mock test suite. "Working" under
the mocks is NOT a claim that real OneCore playback works -- only Windows is.

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
import queue
import subprocess
import tempfile
import threading
import wave
from typing import Optional

from sonara.platform.base import TtsBackend

_BASELINE_WPM: float = 200.0  # Sonara's default wpm maps to SpeakingRate 1.0

_WINRT_INSTALL_HINT = (
    "PyWinRT is not installed, so Sonara cannot synthesize speech. Install it: "
    "pip install winrt-runtime winrt-Windows.Media.SpeechSynthesis "
    "winrt-Windows.Storage.Streams"
)


def _winrt_available() -> bool:
    """True if the OneCore TTS WinRT projection can be imported. Used by run()
    (actionable error) and by `sonara doctor` (so an undeclared/missing PyWinRT
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
    """Map Sonara [100-400] wpm to a SpeakingRate multiplier [0.5-6.0].

    SpeakingRate is a multiplier, not an absolute wpm; values outside
    [0.5, 6.0] raise on real WinRT, so we always clamp.
    """
    return max(0.5, min(6.0, wpm / _BASELINE_WPM))


_TMP_PREFIX = "sonara-tts-"


def _sweep_stale_wavs(max_age_s: float = 300.0) -> None:
    """Best-effort cleanup of temp WAVs leaked by a prior crashed/killed daemon.
    Only removes files older than *max_age_s*, so a clip that another instance
    may still be playing is never deleted, and only our own sonara-tts-* prefix
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


# Sentinel for "producer is finished all chunks". Deliberately distinct from
# None: a synth_one chunk that returns None means "produced nothing, skip
# this chunk" and is simply never queued (see _ChatterboxHandle._produce), so
# a skipped chunk can never be confused with the queue's completion signal.
_DONE = object()


class _ChatterboxHandle:
    """Pipelined, interruptible playback of a chatterbox utterance. tts.run
    returns this BEFORE any synthesis, so the speaker sets it as _current at
    once (a cancel mid-synth then reaches terminate(), closing the synth-gap).
    wait() runs a producer thread that synthesizes chunks ~1-2 ahead into a
    bounded queue while the consumer plays them in order; terminate() aborts
    within one chunk. Fits the say_runner handle contract
    (wait/terminate/poll/returncode).

    self._abort (a threading.Event) is the single source of truth for
    "stop": both the producer and the consumer poll it, and terminate() only
    ever sets it (plus terminating whatever sub-handle is currently playing).
    The producer's queue put() uses a short timeout and re-checks _abort each
    iteration so an aborted consumer can never wedge it waiting for room.
    """

    def __init__(self, text, synth_one, on_play=None, play=None, split=None,
                 chunk_play_timeout=60):
        from sonara import chatterbox
        self._chunks = (split or chatterbox.split_text)(text)
        self._synth_one = synth_one
        self._on_play = on_play
        self._play = play or _play_wav_bytes
        self._chunk_play_timeout = chunk_play_timeout
        self._abort = threading.Event()
        self._q = queue.Queue(maxsize=2)      # synth at most ~2 chunks ahead
        self._producer = None
        self._cur_sub = None
        self.returncode = None

    def _produce(self) -> None:
        for chunk in self._chunks:
            if self._abort.is_set():
                break
            try:
                wav = self._synth_one(chunk)
            except Exception:  # noqa: BLE001 - synth_one owns its own fallback; guard anyway
                wav = None
            if wav is None:
                continue        # produced nothing: skip, do NOT enqueue anything
            put_ok = False
            while not self._abort.is_set():
                try:
                    self._q.put(wav, timeout=0.1)
                    put_ok = True
                    break
                except queue.Full:
                    continue    # consumer is still draining; keep checking abort
            if not put_ok:
                break
        # The _DONE sentinel MUST reach the consumer, so retry until it lands (or
        # we are aborting). Dropping it - the old put(timeout=0.5)+pass - left the
        # consumer spinning on get() forever whenever real (slow) playback kept the
        # maxsize queue full at the moment the producer finished (verified live).
        while not self._abort.is_set():
            try:
                self._q.put(_DONE, timeout=0.1)
                break
            except queue.Full:
                continue   # consumer is still draining; room frees as it pops

    def wait(self, timeout=None) -> int:
        if not self._chunks:
            self.returncode = 0
            return 0
        self._producer = threading.Thread(target=self._produce,
                                          name="sonara-cb-synth", daemon=True)
        self._producer.start()
        played_any = False
        rc = 1
        try:
            while True:
                if self._abort.is_set():
                    break
                try:
                    item = self._q.get(timeout=0.2)
                except queue.Empty:
                    continue
                if self._abort.is_set():
                    break
                if item is _DONE:
                    rc = 0
                    break
                if not played_any and self._on_play is not None:
                    try:
                        self._on_play()
                    except Exception:  # noqa: BLE001 - ducking must never block speech
                        pass
                    played_any = True
                try:
                    sub = self._play(item)
                except Exception:  # noqa: BLE001 - a chunk that fails to play is a
                    # fatal playback error for this utterance: stop (rc stays 1)
                    # instead of busy-looping; finally below still cleans up.
                    break
                self._cur_sub = sub
                try:
                    sub.wait(timeout=self._chunk_play_timeout)
                except Exception:  # noqa: BLE001 - a stuck chunk must not wedge the loop
                    try:
                        sub.terminate()
                    except Exception:  # noqa: BLE001
                        pass
                self._cur_sub = None
        finally:
            # Always runs, even if _play (or anything else above) raised, so a
            # failed chunk can never leave returncode None (poll() reporting
            # "still running" forever) or leak the producer thread.
            self.returncode = rc
            self._abort.set()  # in case rc came from a break above: stop the producer
            # Drain any leftover item so a producer still retrying put() sees room
            # and exits promptly, then join it so no thread is ever leaked.
            try:
                while True:
                    self._q.get_nowait()
            except queue.Empty:
                pass
            if self._producer is not None:
                # The join may TIME OUT by design: synth_one is a blocking worker
                # RPC that does not poll _abort, so a chunk already mid-synth when
                # terminate() lands finishes (bounded by the worker timeout) and
                # the producer self-reaps just after. wait() has already stopped
                # playback, so this is a background tail, not a hang.
                self._producer.join(timeout=1.0)
                self._producer = None
        return self.returncode

    def terminate(self) -> None:
        self._abort.set()
        sub = self._cur_sub
        if sub is not None:
            try:
                sub.terminate()
            except Exception:  # noqa: BLE001
                pass

    def poll(self):
        return self.returncode


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
        """Lazy KokoroEngine -- downloads the ~316 MB model on first Kokoro voice."""
        if self._kokoro is None:
            from sonara import kokoro, paths
            self._kokoro = kokoro.KokoroEngine(paths.SONARA_DIR / "kokoro")
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
        """ABC contract: list of selectable voice NAMES (str) -- the installed
        OneCore voices PLUS the 28 Kokoro neural voices, but only when the optional
        [kokoro] extra is installed (else advertising them would let a user pick a
        voice whose first speak silently fails). Internal callers that need the WinRT
        objects use _all_voice_infos()/_best_voice_info() instead. (#16)"""
        from sonara import kokoro, kokoro_provision
        try:
            native = [v.display_name for v in self._all_voice_infos()]
        except Exception:  # noqa: BLE001 - listing must work even with no winrt
            native = []
        # Advertise the neural voices when the engine is reachable on this machine:
        # importable HERE (is_installed) OR provisioned in the venv (neural_enabled).
        # The CLI runs on system python without the extra while the daemon synthesizes
        # via the venv, so gating on is_installed alone hid them from `sonara voice`.
        neural = kokoro.is_installed() or kokoro_provision.neural_enabled()
        kokoro_voices = list(kokoro.VOICES) if neural else []
        # Chatterbox voices are only advertised when the opt-in venv is provisioned
        # (same reasoning as Kokoro above: an unreachable voice would silently fall
        # back to Kokoro on first speak, which is confusing if the user never opted in).
        from sonara import chatterbox
        cb_voices = chatterbox.list_voices() if chatterbox.is_provisioned() else []
        return native + kokoro_voices + cb_voices

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
        """Resolve a Sonara config voice-NAME (or None) to a VoiceInformation.

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

    def _kokoro_wav(self, text, rate):
        """Kokoro-default WAV bytes, used as the fallback synth path for both the
        whole-utterance case (not-provisioned/gate-miss) and per-chunk fallback."""
        from sonara import kokoro
        kokoro.require_installed()
        return self._get_kokoro().wav_bytes(
            text, kokoro.DEFAULT_VOICE, kokoro.rate_to_speed(rate))

    def _chatterbox_synth_one(self, voice, cfg, rate):
        """Return a synth_one(chunk) -> wav bytes closure: try the worker, fall
        back to the Kokoro default voice per chunk on any ChatterboxError so the
        stream never goes silent. A real failure arms the once-per-run notice and
        logs; the caller has already passed the up-front gate."""
        import sys
        from sonara import chatterbox

        def synth_one(chunk):
            try:
                return chatterbox.CLIENT.synth_wav(chunk, voice, cfg)
            except chatterbox.ChatterboxError as exc:
                print("[chatterbox] fallback: {0}".format(exc),
                      file=sys.stderr, flush=True)
                chatterbox._set_fallback_notice(str(exc))
                return self._kokoro_wav(chunk, rate)
        return synth_one

    def run(self, text: str, voice, rate: int, on_play=None):
        """Synthesize *text* and start async winsound playback, returning a
        _TtsHandle the caller can .wait()/.terminate()/.poll().

        A Kokoro voice (af_heart, af_nicole, ...) is synthesized by the Kokoro
        engine; a Chatterbox voice (cb_default or a registered clip stem) is
        synthesized by the worker, falling back to Kokoro-default on any failure;
        anything else by the native WinRT/OneCore engine. Kokoro names are fixed
        and take precedence, so a user clip that happens to be named like a Kokoro
        voice (e.g. `af_heart.wav`) is ignored by chatterbox routing and still
        speaks through Kokoro. All paths produce WAV bytes played through the same
        winsound handle (so cancel/interrupt, earcon mixing, and cleanup are
        identical).

        *on_play* fires here, AFTER synthesis and right before playback begins:
        Kokoro synthesis of a long text takes seconds, and ducking other apps'
        audio through that silent stretch was audibly wrong. A failing on_play
        must never block speech."""
        from sonara import kokoro, chatterbox
        if (not kokoro.is_kokoro_voice(voice)) and chatterbox.is_chatterbox_voice(voice):
            import sys
            from sonara.config import load_config
            cfg = load_config()
            if not chatterbox.is_provisioned():
                print("[chatterbox] fallback: not provisioned", file=sys.stderr, flush=True)
                chatterbox._set_fallback_notice(
                    "not provisioned (run: sonara voices install chatterbox)")
            else:
                # A chosen Chatterbox voice ALWAYS tries Chatterbox (#49): the old
                # VRAM gate self-sabotaged (the loaded model itself held the VRAM
                # it was gating on, so an idle machine kept dropping to Kokoro).
                # A genuinely broken/OOM worker still surfaces as a per-chunk
                # ChatterboxError and falls back audibly with the once-per-run notice.
                return _ChatterboxHandle(
                    text, self._chatterbox_synth_one(voice, cfg, rate),
                    on_play=on_play,
                    # configurable synth chunk size for pronunciation A/B (#27)
                    split=lambda t: chatterbox.split_text(
                        t, max_chars=chatterbox.chunk_chars(cfg)))
            # fell through: whole-utterance Kokoro (not provisioned)
            data = self._kokoro_wav(text, rate)
            if on_play is not None:
                try:
                    on_play()
                except Exception:  # noqa: BLE001 - ducking must never block speech
                    pass
            return _play_wav_bytes(data)
        if kokoro.is_kokoro_voice(voice):
            try:
                kokoro.require_installed()   # actionable error, not a raw ImportError
                data = self._get_kokoro().wav_bytes(
                    text, voice, kokoro.rate_to_speed(rate))
            except Exception as exc:  # noqa: BLE001 - a dead engine must never
                # leave the user with unexplained error noise (#29: winrt's
                # bundled MSVCP140 poisons onnxruntime when winrt loads first).
                # Fall back to the native WinRT voice and arm the once-per-run
                # spoken notice, mirroring the Chatterbox fallback pattern.
                import sys
                print("[kokoro] fallback to Windows voice: {0!r}".format(exc)[:300],
                      file=sys.stderr, flush=True)
                kokoro._set_fallback_notice(str(exc))
                _require_winrt()
                data = self._synthesize_wav(text, None, rate)  # best WinRT voice
        else:
            _require_winrt()   # actionable error instead of a raw ImportError (#7)
            data = self._synthesize_wav(text, voice, rate)
        if on_play is not None:
            try:
                on_play()
            except Exception:  # noqa: BLE001 - ducking must never block speech
                pass
        return _play_wav_bytes(data)
