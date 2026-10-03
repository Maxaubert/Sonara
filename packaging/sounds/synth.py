"""192 kHz synthesis primitives shared by Sonara's earcon generators.

Original work, Sonara, MIT licence. Pure synthesis (numpy + scipy + wave), no
samples. Voices are rendered at 4x (192 kHz) and decimated to 48 kHz by the
generator that uses them (make_round2.py).

The noise sources draw from one module-level generator (RNG), so a sound
that uses noise depends on every noise draw rendered before it in the same
process: make_round2.render_all reseeds it (seed_rng) and keeps the original
render order for that.
"""
from __future__ import annotations

import wave

import numpy as np
from scipy import signal

SR = 48000  # output rate
OS = 4
FS = SR * OS
SEED = 20261003
RNG = np.random.default_rng(SEED)


def seed_rng() -> None:
    """Restart the shared noise generator, so a second render repeats the first."""
    global RNG
    RNG = np.random.default_rng(SEED)


# ---------------------------------------------------------------- primitives

def n_of(dur: float) -> int:
    return max(1, int(round(dur * FS)))


def tvec(dur: float) -> np.ndarray:
    return np.arange(n_of(dur)) / FS


def note(name: str) -> float:
    """'A4' / 'C#5' / 'Bb3' -> Hz."""
    names = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
    s = names[name[0]]
    rest = name[1:]
    if rest[0] == "#":
        s += 1
        rest = rest[1:]
    elif rest[0] == "b":
        s -= 1
        rest = rest[1:]
    midi = 12 * (int(rest) + 1) + s
    return 440.0 * 2 ** ((midi - 69) / 12)


def glide(f0: float, f1: float, dur: float, shape: float = 1.0) -> np.ndarray:
    """Exponential pitch glide f0 -> f1; shape < 1 moves fast early."""
    u = np.linspace(0, 1, n_of(dur)) ** shape
    return f0 * (f1 / f0) ** u


def osc(freq, dur: float, partials=((1, 1.0),), phase0: float = 0.0) -> np.ndarray:
    """Additive oscillator; partials above 20 kHz are dropped per sample."""
    f = np.broadcast_to(np.asarray(freq, dtype=float), (n_of(dur),))
    ph = 2 * np.pi * np.cumsum(f) / FS + phase0
    out = np.zeros_like(ph)
    for k, a in partials:
        mask = (f * k) < 20000
        out += a * np.sin(k * ph) * mask
    return out


def env(dur: float, attack: float = 0.002, tau: float = 0.05, hold: float = 0.0,
        curve: str = "exp") -> np.ndarray:
    t = tvec(dur)
    e = np.ones_like(t)
    if attack > 0:
        a = t < attack
        e[a] = 0.5 - 0.5 * np.cos(np.pi * t[a] / attack)
    d = t >= attack + hold
    if curve == "exp":
        e[d] = np.exp(-(t[d] - attack - hold) / tau)
    else:  # raised-cosine decay over tau
        u = np.clip((t[d] - attack - hold) / tau, 0, 1)
        e[d] = 0.5 + 0.5 * np.cos(np.pi * u)
    return e


def noise(dur: float) -> np.ndarray:
    return RNG.standard_normal(n_of(dur))


def bandpass(x, fc, q=2.0):
    bw = fc / q
    lo, hi = max(20, fc - bw / 2), min(FS / 2 * 0.95, fc + bw / 2)
    sos = signal.butter(2, [lo, hi], "bandpass", fs=FS, output="sos")
    return signal.sosfilt(sos, x)


def lowpass(x, fc, order=2):
    return signal.sosfilt(signal.butter(order, fc, "lowpass", fs=FS, output="sos"), x)


def highpass(x, fc, order=2):
    return signal.sosfilt(signal.butter(order, fc, "highpass", fs=FS, output="sos"), x)


def modal(f0: float, ratios, amps, taus, dur: float, attack: float = 0.0004) -> np.ndarray:
    """Sum of exponentially decaying sine modes (struck object)."""
    t = tvec(dur)
    out = np.zeros_like(t)
    for r, a, tau in zip(ratios, amps, taus):
        f = f0 * r
        if f >= 20000:
            continue
        out += a * np.sin(2 * np.pi * f * t) * np.exp(-t / tau)
    if attack > 0:
        na = n_of(attack)
        out[:na] *= 0.5 - 0.5 * np.cos(np.linspace(0, np.pi, na))
    return out


def burst(dur: float, fc: float, q: float = 1.5, tau: float = 0.002) -> np.ndarray:
    """Short filtered noise transient (strike / contact)."""
    return bandpass(noise(dur), fc, q) * env(dur, 0.0002, tau)


def mix(total: float, *events) -> np.ndarray:
    """events: (offset_seconds, signal, gain)."""
    out = np.zeros(n_of(total))
    for off, x, g in events:
        i = n_of(off) if off > 0 else 0
        j = min(len(out), i + len(x))
        out[i:j] += g * x[: j - i]
    return out


def sm(*xs):
    """Sum signals of different lengths (zero padded)."""
    n = max(len(x) for x in xs)
    out = np.zeros(n)
    for x in xs:
        out[: len(x)] += x
    return out


def unit(x):
    m = np.abs(x).max()
    return x / m if m > 0 else x


def sweep_noise(dur: float, f0: float, f1: float, q: float = 1.2, shape=None) -> np.ndarray:
    """Noise through a TPT state-variable bandpass whose centre sweeps."""
    n = n_of(dur)
    x = RNG.standard_normal(n)
    fc = glide(f0, f1, dur)
    g = np.tan(np.pi * fc / FS)
    k = 1.0 / q
    a1 = 1 / (1 + g * (g + k))
    ic1 = ic2 = 0.0
    y = np.empty(n)
    for i in range(n):
        v3 = x[i] - ic2
        v1 = a1[i] * ic1 + g[i] * a1[i] * v3
        v2 = ic2 + g[i] * v1
        ic1 = 2 * v1 - ic1
        ic2 = 2 * v2 - ic2
        y[i] = v1
    if shape is None:
        u = np.linspace(0, 1, n)
        shape = np.sin(np.pi * u ** 0.6) ** 2
    return lowpass(y, 10000, 4) * shape


def write_wav(path: str, x: np.ndarray) -> None:
    """48 kHz float in [-1, 1] -> 16-bit mono WAV."""
    pcm = np.clip(np.round(x * 32767), -32768, 32767).astype("<i2")
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())
