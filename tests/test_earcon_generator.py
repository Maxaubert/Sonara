from __future__ import annotations
import math
import struct
import wave
import pathlib
from sonari.platform.windows.earcons.generate import generate_earcon, _EARCON_SPECS


def _hdr(p):
    raw = open(p, "rb").read(44)
    return (raw[0:4], raw[8:12], struct.unpack("<H", raw[20:22])[0],
            struct.unpack("<H", raw[22:24])[0], struct.unpack("<I", raw[24:28])[0],
            struct.unpack("<H", raw[34:36])[0])


def _dominant_freq(p: pathlib.Path, quarter: str = "last") -> float:
    """Return the dominant frequency (Hz) in the first or last quarter of *p*.

    Uses a plain DFT (no numpy) over 2048 samples so this works with stdlib
    only and keeps the test free of third-party dependencies.
    """
    with wave.open(str(p)) as w:
        sr = w.getframerate()
        n_frames = w.getnframes()
        raw = w.readframes(n_frames)

    # Decode little-endian signed 16-bit PCM
    samples = [struct.unpack_from("<h", raw, i * 2)[0] for i in range(n_frames)]

    # Window to analyse: last 25% of the signal (post attack, clear of silence)
    window_size = min(2048, n_frames // 4)
    if quarter == "last":
        window = samples[n_frames - window_size:]
    else:
        window = samples[:window_size]

    # DFT magnitude for positive frequencies up to Nyquist
    n = len(window)
    best_mag = -1.0
    best_k = 0
    for k in range(1, n // 2 + 1):
        re = sum(window[i] * math.cos(2 * math.pi * k * i / n) for i in range(n))
        im = sum(window[i] * math.sin(2 * math.pi * k * i / n) for i in range(n))
        mag = re * re + im * im
        if mag > best_mag:
            best_mag = mag
            best_k = k

    return best_k * sr / n


def test_generate_writes_valid_pcm_wav(tmp_path):
    p = tmp_path / "x.wav"
    generate_earcon(p, 440.0, 0.12)
    riff, wav, fmt, ch, sr, bits = _hdr(p)
    assert riff == b"RIFF" and wav == b"WAVE" and fmt == 1 and ch == 1 and sr == 44100 and bits == 16


def test_all_specs_valid(tmp_path):
    for name, (f, d, wt, f2) in _EARCON_SPECS.items():
        p = tmp_path / (name + ".wav")
        generate_earcon(p, f, d, wave_type=wt, freq2=f2)
        assert p.stat().st_size > 0
        with wave.open(str(p)) as w:
            assert abs(w.getnframes() / w.getframerate() - d) < 1e-3


def test_chirp_end_frequency(tmp_path):
    """The dominant frequency in the last quarter of a chirp must be within
    5% of freq2 — this test would have caught the phase-accumulation bug
    where the naive sin(2π·f_inst·t) formula doubled the sweep rate.
    """
    for name, (freq, dur, wtype, freq2) in _EARCON_SPECS.items():
        if wtype != "chirp":
            continue
        assert freq2 is not None  # guaranteed by spec, but make mypy happy
        p = tmp_path / f"{name}.wav"
        generate_earcon(p, freq, dur, wave_type="chirp", freq2=freq2)
        dom = _dominant_freq(p, quarter="last")
        assert abs(dom - freq2) / freq2 < 0.05, (
            f"Chirp '{name}' end freq {dom:.1f} Hz is not within 5% of "
            f"target {freq2:.1f} Hz — check phase accumulation in generate.py"
        )


def test_earcon_wav_assets_exist_for_every_spec():
    """Every key in _EARCON_SPECS must have a matching .wav asset file in the
    package directory.  This catches a mis-named or missing asset file —
    something the old tautological name-equality check could not catch
    because _EARCON_NAMES is now derived directly from _EARCON_SPECS.keys().
    """
    import pathlib
    from sonari.platform.windows.earcons import generate as _gen_mod
    pkg_dir = pathlib.Path(_gen_mod.__file__).parent
    missing = [
        name
        for name in _EARCON_SPECS
        if not (pkg_dir / f"{name}.wav").exists()
    ]
    assert not missing, (
        f"Missing .wav asset(s) in {pkg_dir}: {missing}\n"
        "Run: python -m sonari.platform.windows.earcons.generate"
    )
