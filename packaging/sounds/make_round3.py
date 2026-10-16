"""Sonara's earcon candidates, round 3 (2026-10-15): the picks.

Round 3 offered 15 restart/flush pairs and 30 question sounds; the owner
picked pair 6's restart sound ("rewind swell into a tick") for restart and
flush, question 4 ("bubble pops rising") for a question and question 1
("marimba up a third") for a finished reply. This module renders those,
exactly as they were heard, on make_pack's primitives.
Original procedural synthesis (Sonara, MIT licence), no samples. Needs
numpy and scipy.

    python make_round3.py [out_dir]   (default: ./out)
"""
from __future__ import annotations

import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
import make_pack as mp  # noqa: E402

SR = mp.SR
SEED = 20261015


def n(sec):
    return int(round(sec * SR))


def t_of(dur):
    return np.arange(n(dur)) / SR


def nav_tick():
    """make_pack nav-3: the tick the nav earcon was until round 3."""
    return mp.tick(2080.0, 700.0, 0.0036)


def noise_sweep(rng, dur, f0, f1, width=0.35, shape=None):
    """Noise whose band centre moves f0 -> f1 (frame-wise spectral mask)."""
    total = n(dur)
    frame, hop = 1024, 256
    out = np.zeros(total + frame)
    win = np.hanning(frame)
    freqs = np.fft.rfftfreq(frame, 1 / SR)
    for start in range(0, total, hop):
        u = start / max(1, total - 1)
        fc = f0 * (f1 / f0) ** u
        mask = np.exp(-0.5 * (np.log(np.maximum(freqs, 1) / fc) / width) ** 2)
        spec = np.fft.rfft(rng.standard_normal(frame) * win) * mask
        out[start:start + frame] += np.fft.irfft(spec, frame) * win
    out = out[:total]
    u = np.linspace(0, 1, total)
    shape = np.sin(np.pi * u ** 0.6) ** 2 if shape is None else shape(u)
    return out / (np.abs(out).max() + 1e-12) * shape


def glide_tone(dur, f0, f1, tau=None, partials=((1, 0.0),), curve=1.0, attack=0.004):
    t = t_of(dur)
    u = (t / dur) ** curve
    f = f0 * (f1 / f0) ** u
    ph = 2 * np.pi * np.cumsum(f) / SR
    x = sum(10 ** (db / 20) * np.sin(k * ph) * (f * k < 0.42 * SR) for k, db in partials)
    return x * mp.env_perc(dur, attack, tau or dur / 3)


def bubble(f0, f1, dur=0.07):
    return glide_tone(dur, f0, f1, tau=dur / 2.5, curve=0.7, attack=0.002)


def seq(items, length, wet=-22.0, rt60=0.3, damp=3500):
    """items: (onset, signal, gain_db)."""
    return mp.add_space(mp.mix(length, *items), wet, rt60=rt60, damp_hz=damp)


def rewind_tick():
    """A reversed noise swell rising into the nav tick (restart and flush)."""
    rng = np.random.default_rng(SEED)
    sw = noise_sweep(rng, 0.16, 1200, 5000, width=0.4, shape=lambda u: u ** 2.5)
    return seq([(0.0, sw, -14), (0.155, nav_tick(), 0)], 0.2)


def marimba(f, dur=0.45, tau=0.09):
    return mp.modal(dur, f, [(1.0, 0.0, 1.0), (3.93, -14.0, 0.35), (9.25, -26.0, 0.15)],
                    tau=tau, drop_cents=0, tail_db=-30)


def marimba_third():
    """Marimba, A5 up a major third (a finished reply)."""
    return seq([(0.0, marimba(880), -1), (0.09, marimba(1108.7), 0)], 0.5, wet=-20)


def bubble_pops():
    """Two bubble pops rising (a question)."""
    return seq([(0.0, bubble(400, 1500, 0.09), 0), (0.08, bubble(700, 1900, 0.07), -4)], 0.2)


# file name -> (design, loudness target: max 100 ms K-weighted LUFS)
PICKS = {
    "rewind_tick.wav": (rewind_tick, -24.0),
    "bubble_pops.wav": (bubble_pops, -15.0),
    "marimba_third.wav": (marimba_third, -15.0),
}


def render_all(out_dir):
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    for name, (design, target) in PICKS.items():
        mp.write_wav(out / name, mp.finish(design(), target))


if __name__ == "__main__":
    render_all(sys.argv[1] if len(sys.argv) > 1 else "out")
