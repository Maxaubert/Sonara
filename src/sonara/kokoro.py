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
import os
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

# --- once-per-run fallback notice (#29), mirrors chatterbox's ------------------
# Armed by the tts backend when Kokoro synthesis fails and the utterance falls
# back to the native WinRT voice; the daemon speaks it exactly once per run so
# a dead engine is announced instead of producing unexplained error noise.
_FALLBACK: list = []


def _set_fallback_notice(reason) -> None:
    _FALLBACK[:] = [reason]


def pop_fallback_notice():
    """The pending fallback reason, or None. Clears it (once-per-read)."""
    return _FALLBACK.pop() if _FALLBACK else None
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
    hard-capped so no sample exceeds *peak*. The same target as the Chatterbox
    worker's _normalize_rms (kept in sync - the worker cannot import sonara),
    so loudness is consistent across engines and voices (#81). Frames below a
    tenth of the loudest frame are pauses and do not count; silent/empty audio
    returns unchanged."""
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


def _download(url: str, dest: Path) -> None:
    """Download to a .tmp then atomic-rename (never leaves a half-written dest)."""
    tmp = dest.with_name(dest.name + ".tmp")
    try:
        urllib.request.urlretrieve(url, tmp)
        tmp.replace(dest)
    except Exception:
        if tmp.exists():
            tmp.unlink(missing_ok=True)
        raise


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
    present (default: download); tests pass a no-op.
    """

    def __init__(self, model_dir, factory=None, ensure=None) -> None:
        self._dir = Path(model_dir)
        self._model_path = self._dir / "kokoro-v1.0.onnx"
        self._voices_path = self._dir / "voices-v1.0.bin"
        self._factory = factory or _default_factory
        self._ensure = ensure if ensure is not None else self._download_models
        self._k = None
        self._lock = Lock()

    def _download_models(self) -> None:
        _ensure_file(self._model_path, _MODEL_URL, _MIN_MODEL_BYTES)
        _ensure_file(self._voices_path, _VOICES_URL, _MIN_VOICES_BYTES)

    def _ensure_loaded(self):
        if self._k is not None:
            return self._k
        with self._lock:
            if self._k is None:
                self._ensure()
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
