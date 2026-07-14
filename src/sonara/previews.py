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
        return backend._get_kokoro().wav_bytes(
            text, voice, kokoro.rate_to_speed(rate))
    if chatterbox.is_chatterbox_voice(voice):
        from sonara.config import load_config
        cfg = load_config()
        return chatterbox.CLIENT.synth_wav(text, voice, cfg)
    return backend._synthesize_wav(text, voice, rate)


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
