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
module imports cleanly off Windows for the mock test suite. "Working" under
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


def _kokoro_was_installed() -> bool:
    """True when this machine has Kokoro: importable here or provisioned in
    the neural venv. A failure of an engine that was never installed is not
    news to announce (E12)."""
    try:
        from sonara import kokoro, kokoro_provision
        return kokoro.is_installed() or kokoro_provision.neural_enabled()
    except Exception:  # noqa: BLE001 - the check must never break speech
        return False


def wpm_to_speaking_rate(wpm: float) -> float:
    """Map Sonara [100-400] wpm to a SpeakingRate multiplier [0.5-6.0].

    SpeakingRate is a multiplier, not an absolute wpm; values outside
    [0.5, 6.0] raise on real WinRT, so we always clamp.
    """
    return max(0.5, min(6.0, wpm / _BASELINE_WPM))


_TMP_PREFIX = "sonara-tts-"

# Speech volume percent (25..200); 100 = bypass. Split delivery (#105 rework):
# the 25..100 range rides the process's OWN Windows audio-session volume, which
# applies INSTANTLY to audio already playing (winsound itself has no volume
# API); the boost above 100 rides digital sample gain, which can only take
# effect on the next synthesized playback. _SESSION_APPLIED tracks the last
# session target that actually stuck: the process has no audio session until
# winsound first plays, so _play_wav_bytes retries after starting playback
# until the target lands.
_VOLUME = [100]
_SESSION_APPLIED = [None]
# The download cool-down (KokoroUnavailable) fails every Kokoro utterance for
# up to 30 minutes: log that fallback once per run, not once per cue.
_COOLDOWN_LOGGED = [False]


def _gain_percent() -> int:
    """The sample-gain half: unity for any volume at or below 100."""
    return max(100, _VOLUME[0])


def _session_target() -> int:
    """The session-volume half: capped at unity (sessions cannot boost)."""
    return min(100, _VOLUME[0])


def _push_session_volume() -> None:
    """Try to land the session target on our own audio session; record success
    so repeat playbacks skip the COM enumeration once it stuck."""
    target = _session_target()
    if _SESSION_APPLIED[0] == target:
        return
    try:
        from sonara.platform.windows.self_volume import apply_self_volume
        if apply_self_volume(target):
            _SESSION_APPLIED[0] = target
    except Exception:  # noqa: BLE001 - volume must never break playback
        pass


def set_volume(percent) -> None:
    try:
        _VOLUME[0] = max(25, min(200, int(percent)))
    except (TypeError, ValueError):
        return
    _SESSION_APPLIED[0] = None    # force a re-push (instant when mid-playback)
    _push_session_volume()


# Alias captured right after definition so WinTtsBackend.set_volume (same name,
# shadowed inside the class body) can still reach this MODULE-level function
# by reference instead of an unqualified name lookup that would recurse into
# the method itself.
_module_set_volume = set_volume


def get_volume() -> int:
    return _VOLUME[0]


def _numpy():
    """numpy when importable (it ships with Kokoro), else None."""
    try:
        import numpy
        return numpy
    except Exception:  # noqa: BLE001 - optional speed-up only
        return None


def _scale_wav(data: bytes, percent: int):
    """Gain a 16-bit PCM WAV by percent/100, hard-clamped to int16. Non-16-bit
    or malformed data returns unchanged: playback must never break for want of
    a volume tweak. Stdlib only (audioop left the stdlib in 3.13)."""
    if percent == 100:
        return data
    import array
    import io
    import wave
    try:
        with wave.open(io.BytesIO(data), "rb") as r:
            if r.getsampwidth() != 2:
                return data
            params = r.getparams()
            frames = r.readframes(r.getnframes())
        gain = percent / 100.0
        np = _numpy()
        if np is not None:
            # E21a: a 30 s Kokoro clip is ~720k samples; the per-sample loop
            # below cost up to a second of dead air before playback.
            x = np.frombuffer(frames, dtype="<i2").astype(np.float64) * gain
            scaled = np.clip(np.trunc(x), -32768, 32767).astype("<i2").tobytes()
        else:
            samples = array.array("h")
            samples.frombytes(frames)
            out = array.array("h", bytes(len(frames)))
            for i, s in enumerate(samples):
                v = int(s * gain)
                if v > 32767:
                    v = 32767
                elif v < -32768:
                    v = -32768
                out[i] = v
            scaled = out.tobytes()
        buf = io.BytesIO()
        with wave.open(buf, "wb") as w:
            w.setparams(params)
            w.writeframes(scaled)
        return buf.getvalue()
    except Exception:  # noqa: BLE001 - never break playback for a volume tweak
        return data


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
    data = _scale_wav(data, _gain_percent())
    import winsound
    fd, path = tempfile.mkstemp(suffix=".wav", prefix=_TMP_PREFIX)
    try:
        os.write(fd, data)
    finally:
        os.close(fd)
    duration = _wav_duration(data)
    try:
        # SND_NODEFAULT (E21b): a missing or locked temp WAV must fail, not
        # play the Windows default 'ding' in place of speech.
        winsound.PlaySound(path, winsound.SND_FILENAME | winsound.SND_ASYNC
                           | winsound.SND_NODEFAULT)
    except Exception:
        try:
            os.unlink(path)
        except OSError:
            pass
        raise
    # The first playback CREATES this process's audio session; land the
    # attenuation target on it now if it has not stuck yet (no-op once applied).
    _push_session_volume()
    return _TtsHandle(path, duration)


class WinTtsBackend(TtsBackend):
    """OneCore TTS via PyWinRT synthesis + winsound playback, with optional Kokoro
    neural voices routed through the same winsound path.

    The SpeechSynthesizer is created ONCE and reused (synthesis is stable). All
    winrt.*/winsound imports are lazy (inside methods)."""

    # M9: one SpeechSynthesizer is shared by the speak loop, the preview
    # builder and settings previews, each on its own thread. Its voice and
    # rate are set per call, so set-voice/rate/synthesize/read run under this
    # lock or one thread's voice lands on another's text. (Class-level too, so
    # a backend built without __init__ still has one.)
    _synth_lock = threading.Lock()

    def __init__(self) -> None:
        self._synth = None         # reused SpeechSynthesizer (lazy)
        self._synth_lock = threading.Lock()
        self._kokoro = None        # lazy KokoroEngine (only if a Kokoro voice is used)
        _sweep_stale_wavs()        # clear temp WAVs leaked by a prior crash (#26)

    def _get_kokoro(self):
        """Lazy KokoroEngine. A missing ~316 MB model downloads in the
        background on first use (M2/E11): Kokoro voices speak with the Windows
        voice until it lands, and the daemon announces the download once."""
        if self._kokoro is None:
            from sonara import kokoro, paths

            def _downloading():
                import sys
                print("[kokoro] downloading the neural voice model",
                      file=sys.stderr, flush=True)
                kokoro._set_download_notice()
            self._kokoro = kokoro.KokoroEngine(
                paths.SONARA_DIR / "kokoro", background=True,
                on_download=_downloading)
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
        """Synthesize *text* to WAV bytes (no playback), STA-safe.

        WinRT's blocking .get() refuses to run on a single-threaded-apartment
        COM thread ("Cannot call blocking method from single-threaded
        apartment") and the daemon's speak-loop thread IS one, so the native
        Windows voice raised on every call from there -- surfaced live by the
        fast-cues override (#60); the Kokoro->Windows fallback had the same
        latent break. Fresh Python threads join the MTA, so the blocking body
        always runs on a short-lived worker with exceptions propagated."""
        result: dict = {}

        def _worker():
            try:
                result["wav"] = self._synthesize_wav_blocking(text, voice, rate)
            except BaseException as exc:  # noqa: BLE001 - re-raised on the caller
                result["err"] = exc

        t = threading.Thread(target=_worker, name="sonara-winrt-synth", daemon=True)
        t.start()
        t.join(60)
        if "err" in result:
            raise result["err"]
        if "wav" not in result:
            raise RuntimeError("WinRT synthesis timed out")
        return result["wav"]

    def _synthesize_wav_blocking(self, text: str, voice, rate: int) -> bytes:
        """The real WinRT synthesis; must run on an MTA-capable thread."""
        from winrt.windows.storage.streams import DataReader

        speaking_rate = wpm_to_speaking_rate(rate)
        resolved_voice = self._resolve_voice(voice)   # raises if no voices
        with self._synth_lock:
            return self._synthesize_locked(text, resolved_voice, speaking_rate,
                                           DataReader)

    def _synthesize_locked(self, text, resolved_voice, speaking_rate,
                           DataReader) -> bytes:
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

    def prewarm(self, rate: int) -> None:
        """Load the Kokoro engine ahead of the first cue (#60) by synthesizing
        a throwaway word in the default Kokoro voice."""
        from sonara import kokoro
        kokoro.require_installed()
        self._get_kokoro().wav_bytes(
            "Ready.", kokoro.DEFAULT_VOICE, kokoro.rate_to_speed(rate))

    def run(self, text: str, voice, rate: int, on_play=None):
        """Synthesize *text* and start async winsound playback, returning a
        _TtsHandle the caller can .wait()/.terminate()/.poll().

        A Kokoro voice (af_heart, af_nicole, ...) is synthesized by the Kokoro
        engine; anything else by the native WinRT/OneCore engine. Both paths
        produce WAV bytes played through the same winsound handle (so
        cancel/interrupt, earcon mixing, and cleanup are identical).

        *on_play* fires here, AFTER synthesis and right before playback begins:
        Kokoro synthesis of a long text takes seconds, and ducking other apps'
        audio through that silent stretch was audibly wrong. A failing on_play
        must never block speech."""
        from sonara import kokoro
        if kokoro.is_kokoro_voice(voice):
            try:
                kokoro.require_installed()   # actionable error, not a raw ImportError
                data = self._get_kokoro().wav_bytes(
                    text, voice, kokoro.rate_to_speed(rate))
            except kokoro.KokoroDownloading:
                # The model is still downloading (M2/E11): the Windows voice
                # stands in, and the once-per-run 'downloading' notice says why.
                _require_winrt()
                data = self._synthesize_wav(text, None, rate)
            except Exception as exc:  # noqa: BLE001 - a dead engine must never
                # leave the user with unexplained error noise (#29: winrt's
                # bundled MSVCP140 poisons onnxruntime when winrt loads first).
                # Fall back to the native WinRT voice and arm the once-per-run
                # spoken notice, but only when Kokoro was actually installed:
                # a default install never had it (E12).
                if _kokoro_was_installed():
                    import sys
                    cooling = isinstance(exc, kokoro.KokoroUnavailable)
                    if not (cooling and _COOLDOWN_LOGGED[0]):
                        print("[kokoro] fallback to Windows voice: {0!r}".format(exc)[:300],
                              file=sys.stderr, flush=True)
                    if cooling:
                        _COOLDOWN_LOGGED[0] = True
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

    def set_volume(self, percent) -> None:
        """ABC contract: push the speech gain to the module-level state that
        _play_wav_bytes reads on every utterance. Calls the alias captured
        right after the module function's definition, not the bare name, so
        this same-named method can never be mistaken for recursing into
        itself."""
        _module_set_volume(percent)


# D7 (found on a real PC): OneCore lists David, Zira and Mark, yet every
# synthesis raises FileNotFoundError because their voice data files under
# %WINDIR%\Speech_OneCore\Engines\TTS are gone (only the shared lexicon is
# left). Not a Sonara bug, so doctor names the cause and the repair.
_MISSING_VOICE_DATA_FIX = (
    "Fix: Settings > Time & language > Speech > Manage voices, remove and add "
    "English (United States) again, or in an elevated prompt run: "
    "DISM /Online /Add-Capability "
    "/CapabilityName:Language.TextToSpeech~~~en-US~0.0.1.0")


def probe_windows_voice(backend=None):
    """(ok, detail) for doctor: synthesize a short phrase with the best
    Windows voice, for real. Listing voices is not enough: a voice can be
    listed while its data is missing (D7). Never raises."""
    try:
        if backend is None:
            if not _winrt_available():
                return False, "needs PyWinRT (see the TTS runtime row)"
            backend = WinTtsBackend()
        names = [v.display_name for v in backend._all_voice_infos()]
    except Exception as exc:  # noqa: BLE001 - doctor must always render
        return False, "could not list voices: {0}".format(exc)
    if not names:
        return False, ("no Windows voices installed. Add one: Settings > Time & "
                       "language > Speech > Add voices")
    try:
        backend._synthesize_wav("Sonara voice check.", None, 200)
    except FileNotFoundError:
        return False, (
            "{0} {1} listed but cannot speak: their voice data is missing from "
            "this PC (synthesis fails with 'file not found'). Kokoro voices are "
            "unaffected. {2}".format(", ".join(names),
                                     "is" if len(names) == 1 else "are",
                                     _MISSING_VOICE_DATA_FIX))
    except Exception as exc:  # noqa: BLE001 - doctor must always render
        return False, "synthesis failed: {0}".format(exc)
    return True, "{0} (synthesis ok)".format(names[0])
