"""Pre-rendered voice previews for the settings page (#38).

Live preview synthesis felt broken for Chatterbox voices (multi-second GPU
render per click). Previews are rendered ONCE into ~/.sonara/previews/ by a
background daemon thread; the page then streams the file instantly via
GET /api/preview-audio, and only falls back to live synthesis while a file
has not been built yet.
"""
from __future__ import annotations

import os
import re
from pathlib import Path


def preview_dir() -> Path:
    from sonara.paths import SONARA_DIR
    return SONARA_DIR / "previews"


def preview_path(voice: str) -> Path:
    """The wav file for *voice*. The name is sanitized so a hostile voice
    string cannot escape the previews directory."""
    safe = re.sub(r"[^A-Za-z0-9._ -]", "_", str(voice)).strip(". ") or "voice"
    return preview_dir() / (safe + ".wav")


def sample_text(voice: str) -> str:
    return ("Hi! This is the {0} voice. "
            "Sonara reads your Claude sessions aloud.").format(voice)


def pad_lead(wav_bytes: bytes, ms: int = 600) -> bytes:
    """Prepend *ms* of silence to a WAV. The first ~half second of playback is
    routinely swallowed (a sleeping Windows audio endpoint waking up, or a
    stream starting late), which clipped the preview's opening word ("Hi!" ->
    "i!", reported live). Silence up front means nothing speakable can be
    eaten. Returns the input unchanged on any parse failure."""
    import io
    import wave
    try:
        with wave.open(io.BytesIO(wav_bytes), "rb") as r:
            params = r.getparams()
            frames = r.readframes(r.getnframes())
        pad = b"\x00" * (int(params.framerate * ms / 1000)
                         * params.nchannels * params.sampwidth)
        buf = io.BytesIO()
        with wave.open(buf, "wb") as w:
            w.setparams(params)
            w.writeframes(pad + frames)
        return buf.getvalue()
    except Exception:  # noqa: BLE001 - a pad failure must never lose the preview
        return wav_bytes


def synth_wav(voice: str, rate: int = 200) -> bytes:
    """Render the sample for *voice* to WAV bytes, routed by engine exactly
    like live speech (Kokoro / Chatterbox / WinRT). Uses the platform backend's
    synth internals deliberately: previews must sound identical to the real
    voice path."""
    from sonara.platform import get_platform
    from sonara import kokoro, chatterbox
    backend = get_platform().tts
    text = sample_text(voice)
    if kokoro.is_kokoro_voice(voice):
        data = backend._get_kokoro().wav_bytes(
            text, voice, kokoro.rate_to_speed(rate))
    elif chatterbox.is_chatterbox_voice(voice):
        from sonara.config import load_config
        cfg = load_config()
        data = chatterbox.CLIENT.synth_wav(text, voice, cfg)
    else:
        data = backend._synthesize_wav(text, voice, rate)
    return pad_lead(data)


def ensure_all(voices_by_engine: dict, synth=synth_wav, log=None) -> int:
    """Generate any MISSING preview files for the given voices (the
    /api/state grouping: {"windows": [...], "kokoro": [...], ...}). Existing
    files are never re-rendered. Failures skip that voice (no empty files);
    generation must never take anything else down. Returns how many files
    were written."""
    made = 0
    try:
        preview_dir().mkdir(parents=True, exist_ok=True)
    except OSError:
        return 0
    for names in (voices_by_engine or {}).values():
        for voice in names or []:
            path = preview_path(voice)
            if path.exists():
                continue
            try:
                data = synth(voice)
            except Exception as exc:  # noqa: BLE001 - one bad voice must not stop the rest
                if log:
                    log("preview synth failed for {0!r}: {1!r}".format(voice, exc))
                continue
            if not data:
                continue
            tmp = str(path) + ".tmp"
            try:
                with open(tmp, "wb") as fh:
                    fh.write(data)
                os.replace(tmp, str(path))
                made += 1
            except OSError as exc:
                if log:
                    log("preview write failed for {0!r}: {1!r}".format(voice, exc))
    return made
