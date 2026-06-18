"""Pure-stdlib earcon generator.  No third-party dependencies.

WAV format produced: RIFF/WAVE, PCM (AudioFormat=1), 16-bit signed
Little-Endian, mono, 44100 Hz.  This is the exact format winsound
requires; it is also accepted by macOS AudioToolbox and Linux ALSA.

Typical use — run once from the repo root to regenerate assets::

    python -m sonari.platform.windows.earcons.generate \\
        src/sonari/platform/windows/earcons
"""
from __future__ import annotations

import math
import pathlib
import struct
import wave


_SAMPLE_RATE = 44100  # Hz — universally accepted by winsound / Windows
_AMPLITUDE   = 28000  # out of 32767 (16-bit max); leaves headroom
_ATTACK_S    = 0.010  # 10 ms linear attack
_RELEASE_S   = 0.010  # 10 ms linear release


def generate_earcon(
    path: "str | pathlib.Path",
    freq: float,
    duration: float,
    *,
    sample_rate: int = _SAMPLE_RATE,
    wave_type: str = "sine",   # "sine" | "dual" | "chirp"
    freq2: "float | None" = None,
) -> None:
    """Write a short earcon .wav file.

    Parameters
    ----------
    path:        destination file path (parents must exist)
    freq:        fundamental frequency in Hz
    duration:    length in seconds
    sample_rate: default 44100 (required by winsound on Windows)
    wave_type:   "sine"  — pure sine at *freq*
                 "dual"  — 60% *freq* + 40% *freq2* (two-tone blend)
                 "chirp" — linear freq sweep from *freq* to *freq2*
    freq2:       second frequency; required for "dual" and "chirp"
    """
    n           = int(sample_rate * duration)
    attack_n    = int(_ATTACK_S  * sample_rate)
    release_n   = int(_RELEASE_S * sample_rate)
    frames: "list[bytes]" = []

    # Chirp-only constants — hoisted out of the loop so they are computed
    # once per call rather than once per sample (O(1) vs O(n)).
    # _chirp_phi is the phase accumulator; it is only used when
    # wave_type == "chirp" and is meaningless for sine / dual.
    if wave_type == "chirp":
        _chirp_denom: int = max(n - 1, 1)
        _chirp_phi: float = 0.0

    for i in range(n):
        t = i / sample_rate

        # Trapezoid amplitude envelope — removes click at start/end
        if i < attack_n:
            env = i / attack_n
        elif i >= n - release_n:
            env = (n - i) / release_n
        else:
            env = 1.0

        if wave_type == "sine":
            v = math.sin(2 * math.pi * freq * t)

        elif wave_type == "dual":
            if freq2 is None:
                raise ValueError("freq2 required for wave_type='dual'")
            v = 0.6 * math.sin(2 * math.pi * freq  * t) \
              + 0.4 * math.sin(2 * math.pi * freq2 * t)

        elif wave_type == "chirp":
            if freq2 is None:
                raise ValueError("freq2 required for wave_type='chirp'")
            # Instantaneous frequency sweeps linearly from freq to freq2.
            # Phase accumulation gives the correct result; the naive
            # sin(2π·f_inst·t) formula doubles the sweep rate because
            # both f_inst and t grow with i.
            f_inst = freq + (freq2 - freq) * (i / _chirp_denom)
            _chirp_phi += 2 * math.pi * f_inst / sample_rate
            v = math.sin(_chirp_phi)

        else:
            raise ValueError(f"Unknown wave_type: {wave_type!r}")

        sample = max(-32767, min(32767, int(v * env * _AMPLITUDE)))
        frames.append(struct.pack("<h", sample))  # little-endian signed 16-bit

    with wave.open(str(path), "w") as w:
        w.setnchannels(1)          # mono
        w.setsampwidth(2)          # 16-bit = 2 bytes
        w.setframerate(sample_rate)
        w.writeframes(b"".join(frames))


# ---------------------------------------------------------------------------
# The 6 canonical Sonari earcons
# ---------------------------------------------------------------------------
_EARCON_SPECS: "dict[str, tuple]" = {
    # name         freq   dur   wave_type  freq2
    "permission": (440.0, 0.12, "sine",    None ),  # A4 — clean, neutral ask
    "choice":     (660.0, 0.15, "dual",    880.0),  # E5+A5 — bright two-tone
    "plan":       (528.0, 0.20, "chirp",   660.0),  # C5→E5 rising sweep
    "error":      (220.0, 0.25, "dual",    185.0),  # low dissonant pair
    "turn_done":  (880.0, 0.10, "sine",    None ),  # A5 — short, high
    "ready":      (523.0, 0.18, "chirp",   784.0),  # C5→G5 ascending
}


def generate_all_earcons(output_dir: "str | pathlib.Path") -> None:
    """Write all 6 earcon .wav files into *output_dir*.

    Idempotent — safe to call multiple times; overwrites existing files.
    Typical use: run once from the repo root to regenerate assets::

        python -m sonari.platform.windows.earcons.generate \\
            src/sonari/platform/windows/earcons
    """
    output_dir = pathlib.Path(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    for name, (freq, dur, wtype, freq2) in _EARCON_SPECS.items():
        generate_earcon(
            output_dir / f"{name}.wav",
            freq, dur,
            wave_type=wtype,
            freq2=freq2,
        )


if __name__ == "__main__":
    import sys
    dest = sys.argv[1] if len(sys.argv) > 1 else str(pathlib.Path(__file__).parent)
    generate_all_earcons(dest)
    print(f"Generated {len(_EARCON_SPECS)} earcons in {dest}")
