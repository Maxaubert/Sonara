"""Kokoro-82M neural TTS engine (optional, cross-platform).

A portable wrapper around the `kokoro-onnx` package: one ~310 MB ONNX model plus a
~6 MB voices file (`voices-v1.0.bin`) provide ALL 28 voices. This module only
synthesizes to audio / WAV bytes; playback is the platform TTS backend's job (it
plays the WAV through its existing path -- winsound on Windows).

Voices are selected by bare name (`af_heart`) or the engine-prefixed form
(`kokoro:af_heart`). A voice not in VOICES is not ours -- the caller routes it to
the native engine. Everything heavy (kokoro_onnx / onnxruntime / numpy) imports
lazily, so importing this module never pulls the ML stack in; it's only loaded the
first time a Kokoro voice is actually spoken (declared as the `[kokoro]` extra).
"""
from __future__ import annotations

import importlib.util
import io
import threading
import time
import urllib.request
import wave
from pathlib import Path
from threading import Lock

# All 28 Kokoro voices (af_=US female, am_=US male, bf_=GB female, bm_=GB male).
# Order/grades mirror the upstream catalog; af_heart is the top-rated default and
# af_nicole is the ASMR/whisper voice.
VOICES = [
    "af_heart", "af_bella", "bf_emma", "af_nicole", "af_aoede", "af_kore",
    "af_sarah", "am_fenrir", "am_michael", "am_puck", "af_alloy", "af_nova",
    "bf_isabella", "bm_fable", "bm_george", "af_sky", "bm_lewis", "af_jessica",
    "af_river", "am_echo", "am_eric", "am_liam", "am_onyx", "bf_alice",
    "bf_lily", "bm_daniel", "am_santa", "am_adam",
]

DEFAULT_VOICE = "af_heart"

# --- once-per-run fallback notice (#29) -----------------------------------------
# Armed by the tts backend when Kokoro synthesis fails and the utterance falls
# back to the native WinRT voice; the daemon speaks it exactly once per run so
# a dead engine is announced instead of producing unexplained error noise.
_FALLBACK: list = []


def _set_fallback_notice(reason) -> None:
    _FALLBACK[:] = [reason]


def pop_fallback_notice():
    """The pending fallback reason, or None. Clears it (once-per-read)."""
    return _FALLBACK.pop() if _FALLBACK else None


# --- once-per-run 'downloading' notice (M2/E11, upstream #53) --------------------
# Armed when the one-time model download starts; the daemon speaks it once so
# the Windows voice standing in meanwhile is explained, not a silent mystery.
_DOWNLOADING: list = []


def _set_download_notice() -> None:
    _DOWNLOADING[:] = [True]


def pop_download_notice() -> bool:
    """True once after a model download started. Clears it."""
    return bool(_DOWNLOADING.pop()) if _DOWNLOADING else False


class KokoroUnavailable(RuntimeError):
    """Kokoro cannot synthesize right now (a recent download failed). The
    caller falls back to the native voice."""


class KokoroDownloading(KokoroUnavailable):
    """The model download is running in the background. The caller speaks
    with the native voice meanwhile; the 'downloading' notice explains it."""


_SAMPLE_RATE = 24000     # Kokoro outputs 24 kHz

_MODEL_URL = (
    "https://github.com/thewh1teagle/kokoro-onnx/releases/download/"
    "model-files-v1.0/kokoro-v1.0.onnx"
)
_VOICES_URL = (
    "https://github.com/thewh1teagle/kokoro-onnx/releases/download/"
    "model-files-v1.0/voices-v1.0.bin"
)
_MIN_MODEL_BYTES = 100_000_000   # ~310 MB real; floor well below
_MIN_VOICES_BYTES = 1_000_000    # ~6 MB real

# A stalled connection gives up after this many seconds without data, instead
# of hanging forever (urlretrieve had no timeout at all, M2).
DOWNLOAD_TIMEOUT_S = 30
# After a failed download, Kokoro is skipped (native voice) for this long
# rather than re-fetching 316 MB on every utterance (E11). The memo is a file
# in the model dir, so a daemon restart honours it too.
DOWNLOAD_RETRY_S = 30 * 60
_FAILED_MEMO = ".download_failed"
_CHUNK = 1024 * 1024


def normalize_voice(name) -> str:
    """Strip an optional `kokoro:` engine prefix and lowercase. '' for None."""
    if not name:
        return ""
    s = str(name).strip()
    if ":" in s:
        engine, _, rest = s.partition(":")
        if engine.strip().lower() == "kokoro":
            s = rest.strip()
    return s.lower()


def is_kokoro_voice(name) -> bool:
    """True if *name* (bare or `kokoro:`-prefixed) is one of the 28 Kokoro voices."""
    return normalize_voice(name) in VOICES


def rate_to_speed(rate) -> float:
    """Map a Sonara WPM-style rate (≈100–400, 200 = normal) to Kokoro's speed
    multiplier (pitch-preserving time-stretch), clamped to a sane 0.5–2.0."""
    try:
        speed = float(rate) / 200.0
    except (TypeError, ValueError):
        return 1.0
    return max(0.5, min(2.0, speed))


# The optional [kokoro] extra: kokoro_onnx pulls onnxruntime; numpy is used by
# to_wav_bytes/synth. find_spec checks importability WITHOUT importing the heavy
# stack, so this stays cheap enough to call from list_voices().
_EXTRA_MODULES = ("numpy", "kokoro_onnx")

_INSTALL_HINT = (
    "The optional Kokoro neural-TTS engine is not installed, so its voices cannot "
    "synthesize. Install the extra: pip install 'sonara[kokoro]'"
)


def is_installed() -> bool:
    """True if the optional [kokoro] extra is importable. Gates voice listing and
    the actionable require_installed() check -- kept cheap via find_spec (no import)."""
    try:
        return all(importlib.util.find_spec(m) is not None for m in _EXTRA_MODULES)
    except (ImportError, ValueError):
        return False


def require_installed() -> None:
    """Raise an actionable RuntimeError if the [kokoro] extra is absent -- instead of
    the raw ModuleNotFoundError that the daemon's speak loop would swallow into
    silent no-speech. Mirrors the WinRT backend's _require_winrt() (#7)."""
    if not is_installed():
        raise RuntimeError(_INSTALL_HINT)


def normalize_rms(audio, target=0.08, peak=0.97, frame=480, floor=1e-4):
    """Scale float mono audio so its VOICED RMS lands on *target* (~-22 dBFS),
    hard-capped so no sample exceeds *peak*, so loudness is consistent across
    voices (#81). Frames below a tenth of the loudest frame are pauses and do
    not count; silent/empty audio returns unchanged."""
    import numpy as np
    x = np.asarray(audio, dtype=np.float32)
    if x.size < frame:
        return x
    frames = x[: (len(x) // frame) * frame].reshape(-1, frame)
    rms = np.sqrt((frames ** 2).mean(axis=1))
    gate = max(floor, float(rms.max()) * 0.1)
    voiced = rms[rms > gate]
    cur = float(voiced.mean()) if voiced.size else 0.0
    if cur <= floor:
        return x
    gain = target / cur
    peak_now = float(np.abs(x).max())
    if peak_now * gain > peak:
        gain = peak / peak_now
    return (x * gain).astype(np.float32)


def to_wav_bytes(audio, sample_rate: int = _SAMPLE_RATE) -> bytes:
    """Encode a float32 [-1, 1] mono array as 16-bit PCM mono WAV bytes."""
    import numpy as np
    arr = np.asarray(audio, dtype=np.float32)
    pcm = (np.clip(arr, -1.0, 1.0) * 32767.0).astype("<i2").tobytes()
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(int(sample_rate))
        w.writeframes(pcm)
    return buf.getvalue()


def _download(url: str, dest: Path, timeout: float = DOWNLOAD_TIMEOUT_S) -> None:
    """Download to a .tmp then atomic-rename (never leaves a half-written dest).
    Chunked, with a socket timeout, so a stalled transfer raises instead of
    hanging the caller forever."""
    tmp = dest.with_name(dest.name + ".tmp")
    try:
        with urllib.request.urlopen(url, timeout=timeout) as resp, \
                open(str(tmp), "wb") as out:
            while True:
                chunk = resp.read(_CHUNK)
                if not chunk:
                    break
                out.write(chunk)
        tmp.replace(dest)
    except Exception:
        if tmp.exists():
            tmp.unlink(missing_ok=True)
        raise


def _memo_path(model_dir) -> Path:
    return Path(model_dir) / _FAILED_MEMO


def download_failed_at(model_dir):
    """Epoch seconds of the last failed model download, or None."""
    try:
        return float(_memo_path(model_dir).read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return None


def _remember_failure(model_dir, when: float) -> None:
    try:
        Path(model_dir).mkdir(parents=True, exist_ok=True)
        _memo_path(model_dir).write_text(str(when), encoding="utf-8")
    except OSError:
        pass


def clear_download_failure(model_dir) -> None:
    try:
        _memo_path(model_dir).unlink()
    except OSError:
        pass


def models_present(model_dir) -> bool:
    """True when both model files are on disk at a plausible size."""
    d = Path(model_dir)
    for name, floor in (("kokoro-v1.0.onnx", _MIN_MODEL_BYTES),
                        ("voices-v1.0.bin", _MIN_VOICES_BYTES)):
        try:
            if (d / name).stat().st_size < floor:
                return False
        except OSError:
            return False
    return True


def _ensure_file(dest: Path, url: str, min_bytes: int) -> None:
    """Ensure *dest* exists and is at least *min_bytes* (else (re)download)."""
    if dest.exists():
        try:
            if dest.stat().st_size >= min_bytes:
                return
        except OSError:
            pass
        dest.unlink(missing_ok=True)
    dest.parent.mkdir(parents=True, exist_ok=True)
    _download(url, dest)


def _default_factory(model_path: str, voices_path: str):
    """Build a real kokoro_onnx.Kokoro (lazy import of the ML stack)."""
    from kokoro_onnx import Kokoro
    return Kokoro(model_path, voices_path)


class KokoroEngine:
    """Lazily downloads + loads the Kokoro model and synthesizes audio.

    *factory* builds the underlying engine from (model_path, voices_path) -- the
    default uses kokoro_onnx; tests inject a fake. *ensure* makes the model files
    present (default: download); tests pass a no-op. *present* reports whether
    they already are (default: both files on disk).

    The download never runs under ``_lock`` (M2): a stalled fetch must not hold
    up every synth. With *background* (the daemon's backend), a missing model
    is fetched on a worker thread and synth raises KokoroDownloading at once,
    so the caller speaks with the native voice meanwhile. *on_download* fires
    once when a fetch starts. A failed fetch is remembered on disk and not
    retried for DOWNLOAD_RETRY_S (KokoroUnavailable), unless forced.
    """

    def __init__(self, model_dir, factory=None, ensure=None, *,
                 present=None, background=False, on_download=None,
                 clock=time.time) -> None:
        self._dir = Path(model_dir)
        self._model_path = self._dir / "kokoro-v1.0.onnx"
        self._voices_path = self._dir / "voices-v1.0.bin"
        self._factory = factory or _default_factory
        self._ensure = ensure if ensure is not None else self._download_models
        self._present = present if present is not None else (
            lambda: models_present(self._dir))
        self._background = background
        self._on_download = on_download
        self._clock = clock
        self._k = None
        self._lock = Lock()           # engine load only, never the download
        self._dl_lock = Lock()        # one download at a time
        self._download_thread = None

    def _download_models(self) -> None:
        _ensure_file(self._model_path, _MODEL_URL, _MIN_MODEL_BYTES)
        _ensure_file(self._voices_path, _VOICES_URL, _MIN_VOICES_BYTES)

    def _check_cooldown(self) -> None:
        failed = download_failed_at(self._dir)
        if failed is None:
            return
        left = DOWNLOAD_RETRY_S - (self._clock() - failed)
        if left > 0:
            raise KokoroUnavailable(
                "The neural voice download failed recently; Sonara retries in "
                "about {0} min. To retry now: sonara voices install".format(
                    max(1, int(left // 60))))

    def download_models(self, force: bool = False) -> None:
        """Fetch the model files now, on this thread. *force* ignores (and on
        success clears) a recent failure, for an explicit `voices install`."""
        with self._dl_lock:
            if self._present():
                clear_download_failure(self._dir)
                return
            if not force:
                self._check_cooldown()
            if self._on_download is not None:
                try:
                    self._on_download()
                except Exception:  # noqa: BLE001 - a status cue must never stop it
                    pass
            try:
                self._ensure()
            except BaseException:
                _remember_failure(self._dir, self._clock())
                raise
            clear_download_failure(self._dir)

    def _start_background_download(self) -> None:
        if self._dl_lock.locked() or (
                self._download_thread is not None and self._download_thread.is_alive()):
            raise KokoroDownloading("The neural voice is still downloading.")
        self._check_cooldown()

        def _run():
            try:
                self.download_models()
            except Exception as exc:  # noqa: BLE001 - memo written; log only
                import sys
                print("[kokoro] model download failed: {0!r}".format(exc)[:300],
                      file=sys.stderr, flush=True)
        t = threading.Thread(target=_run, name="sonara-kokoro-download", daemon=True)
        self._download_thread = t
        t.start()
        raise KokoroDownloading("The neural voice is downloading.")

    def _ensure_loaded(self):
        if self._k is not None:
            return self._k
        if not self._present():
            if self._background:
                self._start_background_download()
            else:
                self.download_models()
        with self._lock:
            if self._k is None:
                self._k = self._factory(str(self._model_path), str(self._voices_path))
        return self._k

    def synth(self, text: str, voice: str, speed: float = 1.0):
        """Synthesize *text* with *voice* at *speed*. Returns (float32 audio, sr).

        kokoro_onnx batches phonemes to a 510-token cap, but its splitter can
        emit an over-long batch on unusual text (multi-paragraph summary
        digests hit this live), and the model then raises IndexError: index
        510 out of bounds. Recover by bisecting the text at a whitespace
        boundary and synthesizing the halves; a single unsplittable chunk
        re-raises so the speak loop's failure path takes over."""
        k = self._ensure_loaded()
        v = normalize_voice(voice) or DEFAULT_VOICE
        return self._synth_split(k, text, v, float(speed))

    def _synth_split(self, k, text: str, voice: str, speed: float):
        try:
            audio, sample_rate = k.create(text, voice=voice, speed=speed,
                                          lang="en-us")
            return audio, int(sample_rate)
        except IndexError:
            mid = len(text) // 2
            # Split at the whitespace nearest the middle so words stay whole.
            left = text.rfind(" ", 0, mid)
            right = text.find(" ", mid)
            cut = left
            if right != -1 and (cut == -1 or (mid - left) > (right - mid)):
                cut = right
            if cut in (-1, 0) or cut >= len(text) - 1:
                raise                    # nothing to bisect: genuine failure
            head, tail = text[:cut].strip(), text[cut:].strip()
            if not head or not tail:
                raise
            import numpy as np
            a1, sr = self._synth_split(k, head, voice, speed)
            a2, _ = self._synth_split(k, tail, voice, speed)
            return np.concatenate([a1, a2]), int(sr)

    def wav_bytes(self, text: str, voice: str, speed: float = 1.0) -> bytes:
        """Synthesize and encode straight to 16-bit PCM mono WAV bytes,
        loudness-normalized to the shared cross-engine target (#81)."""
        audio, sample_rate = self.synth(text, voice, speed)
        return to_wav_bytes(normalize_rms(audio), sample_rate)
