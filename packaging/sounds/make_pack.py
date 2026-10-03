"""Sonara's earcon candidates, round 1 (all eight kinds, six variants each).

Every sound is synthesised from first principles (additive sines, two-operator FM,
modal resonators and a seeded noise-tail reverb) with numpy; no samples or external
audio content are used, so the output is original work (Sonara, MIT licence).
Output: 48 kHz, 16-bit mono WAV.

Usage: python make_pack.py [out_dir]      (default: ./out)
"""
from __future__ import annotations

import sys
import wave
from pathlib import Path

import numpy as np

SR = 48000
NYQ_SAFE = 0.42 * SR  # partials above this are dropped (no aliasing)


# ---------------------------------------------------------------- primitives

def n_of(sec):
    return int(round(sec * SR))


def tvec(dur):
    return np.arange(n_of(dur)) / SR


def raised_cos_in(n):
    if n <= 1:
        return np.ones(max(n, 0))
    return 0.5 - 0.5 * np.cos(np.pi * np.arange(n) / n)


def env_perc(dur, attack, tau, hold=0.0, curve=1.0):
    """Raised-cosine attack, optional hold, exponential decay (amplitude time constant tau)."""
    t = tvec(dur)
    e = np.ones_like(t)
    na = max(1, n_of(attack))
    e[:na] = raised_cos_in(na) ** curve
    td = t - attack - hold
    m = td > 0
    e[m] *= np.exp(-td[m] / tau)
    nf = min(n_of(0.012), len(e) // 4)  # end fade: a note never stops abruptly
    if nf > 1:
        e[-nf:] *= raised_cos_in(nf)[::-1]
    return e


def sine(dur, f, glide_cents=0.0, glide_tau=0.02, vib_hz=0.0, vib_cents=0.0, phase=0.0):
    """Sine whose pitch starts glide_cents away and settles exponentially to f."""
    t = tvec(dur)
    cents = glide_cents * np.exp(-t / glide_tau) if glide_cents else np.zeros_like(t)
    if vib_cents:
        cents = cents + vib_cents * np.sin(2 * np.pi * vib_hz * t)
    fi = f * 2 ** (cents / 1200)
    if fi.max() > NYQ_SAFE:
        return np.zeros_like(t)
    ph = phase + 2 * np.pi * np.cumsum(fi) / SR
    return np.sin(ph)


def additive(dur, f0, partials, attack, tau, glide_cents=0.0, glide_tau=0.02, hold=0.0,
             vib_hz=0.0, vib_cents=0.0):
    """partials: list of (ratio, amp_db, tau_mult). Each partial has its own decay."""
    dur = max(dur, attack + hold + 6.9 * tau * max(p[2] for p in partials))  # ring out to -60 dB
    out = np.zeros(n_of(dur))
    for ratio, amp_db, tmul in partials:
        f = f0 * ratio
        if f > NYQ_SAFE:
            continue
        s = sine(dur, f, glide_cents, glide_tau, vib_hz, vib_cents)
        out += 10 ** (amp_db / 20) * s * env_perc(dur, attack, tau * tmul, hold)
    return out


def fm_epiano(dur, f0, attack=0.003, tau=0.18, index=0.55, index_tau=0.035,
              tine=((13.6, -18, 0.012), (17.3, -23, 0.008)), body_db=-24):
    """Two-operator FM (ratio 1) with a decaying index plus short inharmonic 'tine' sines.
    Bandwidth stays under ~(index+2)*f0, far below Nyquist for the registers used."""
    dur = max(dur, attack + 6.9 * tau)
    t = tvec(dur)
    I = index * np.exp(-t / index_tau) + 0.08
    mod = I * np.sin(2 * np.pi * f0 * t)
    car = np.sin(2 * np.pi * f0 * t + mod)
    e = env_perc(dur, attack, tau)
    sig = car * e
    sig += 10 ** (body_db / 20) * np.sin(2 * np.pi * 2 * f0 * t) * env_perc(dur, attack, tau * 0.5)
    for ratio, db, ttau in tine:
        f = f0 * ratio
        while f > 11000.0:  # keep the tine sparkle below ~11 kHz: bright, never piercing
            f /= 2
        sig += 10 ** (db / 20) * np.sin(2 * np.pi * f * t) * env_perc(dur, 0.0015, ttau)
    return sig


def modal(dur, f0, modes, attack=0.0015, tau=0.06, drop_cents=60, drop_tau=0.012,
          beat_hz=0.0, beat_db=-10, tail_db=-20, tail_tau_mult=2.6):
    """Struck-body 'bonk': inharmonic decaying modes, small pitch drop on impact, optional
    detuned twin of the fundamental (slow beating) and a quieter long tail (two-stage decay)."""
    dur = max(dur, attack + 6.9 * tau * max([m[2] for m in modes] + [tail_tau_mult * 0.7, 1.3]))
    out = np.zeros(n_of(dur))
    for ratio, db, tmul in modes:
        f = f0 * ratio
        if f > NYQ_SAFE:
            continue
        s = sine(dur, f, drop_cents, drop_tau)
        out += 10 ** (db / 20) * s * env_perc(dur, attack, tau * tmul)
    s = sine(dur, f0, drop_cents, drop_tau)
    out += 10 ** (tail_db / 20) * s * env_perc(dur, attack * 3, tau * tail_tau_mult)
    if beat_hz:
        s = sine(dur, f0 + beat_hz, drop_cents, drop_tau)
        out += 10 ** (beat_db / 20) * s * env_perc(dur, attack, tau * 1.3)
    return out


def tick(f_hi, f_lo, dur, lo_db=-9.0, hi_db=0.0):
    """Hann-windowed two-partial tick: click-free, band-limited."""
    t = tvec(dur)
    w = np.hanning(len(t) + 2)[1:-1]
    s = 10 ** (hi_db / 20) * np.sin(2 * np.pi * f_hi * t)
    if f_lo:
        s += 10 ** (lo_db / 20) * np.sin(2 * np.pi * f_lo * t + 0.6)
    return s * w


def one_pole_lp(x, fc):
    a = np.exp(-2 * np.pi * fc / SR)
    from scipy.signal import lfilter
    return lfilter([1 - a], [1, -a], x)


_IR_CACHE = {}


def reverb_ir(rt60, damp_hz, predelay=0.008, seed=7):
    key = (rt60, damp_hz, predelay, seed)
    if key in _IR_CACHE:
        return _IR_CACHE[key]
    rng = np.random.default_rng(seed)
    dur = rt60 * 1.1
    t = tvec(dur)
    noise = rng.standard_normal(len(t))
    # frequency-dependent decay: blend a bright fast-decaying and a dark slower tail
    dark = one_pole_lp(noise, damp_hz)
    bright = noise - one_pole_lp(noise, 2500.0)
    k = 6.91 / rt60
    ir = dark * np.exp(-k * t) + 0.25 * bright * np.exp(-2.2 * k * t)
    ir[: n_of(0.002)] *= raised_cos_in(n_of(0.002))
    ir /= np.sqrt((ir ** 2).sum())
    ir = np.concatenate([np.zeros(n_of(predelay)), ir])
    _IR_CACHE[key] = ir
    return ir


def add_space(x, wet_db, rt60=0.45, damp_hz=4000.0):
    if wet_db is None:
        return x
    ir = reverb_ir(rt60, damp_hz)
    pad = np.concatenate([x, np.zeros(len(ir))])
    wet = np.fft.irfft(np.fft.rfft(pad, 2 * len(pad)) * np.fft.rfft(ir, 2 * len(pad)))[: len(pad)]
    # wet level relative to dry energy
    g = 10 ** (wet_db / 20) * np.sqrt((x ** 2).sum() / ((wet ** 2).sum() + 1e-20))
    return pad + g * wet


def mix(length, *events):
    """events: (onset_seconds, signal, gain_db)."""
    total = max([n_of(length)] + [n_of(at) + len(sig) for at, sig, _ in events])
    out = np.zeros(total)  # length is a minimum: notes are never cut off
    for at, sig, db in events:
        i = n_of(at)
        out[i:i + len(sig)] += 10 ** (db / 20) * sig
    return out


# ---------------------------------------------------------------- loudness / finishing

def k_weight(x):
    from scipy.signal import lfilter
    f0, G, Q = 1681.974450955533, 3.999843853973347, 0.7071752369554196
    K = np.tan(np.pi * f0 / SR)
    Vh = 10 ** (G / 20)
    Vb = Vh ** 0.4996667741545416
    a0 = 1 + K / Q + K * K
    b1 = [(Vh + Vb * K / Q + K * K) / a0, 2 * (K * K - Vh) / a0, (Vh - Vb * K / Q + K * K) / a0]
    a1 = [1, 2 * (K * K - 1) / a0, (1 - K / Q + K * K) / a0]
    f0, Q = 38.13547087602444, 0.5003270373238773
    K = np.tan(np.pi * f0 / SR)
    a2 = [1, 2 * (K * K - 1) / (1 + K / Q + K * K), (1 - K / Q + K * K) / (1 + K / Q + K * K)]
    return lfilter([1, -2, 1], a2, lfilter(b1, a1, x))


def loud_s100(x):
    y = k_weight(x) ** 2
    n = n_of(0.1)
    y = np.concatenate([np.zeros(n), y, np.zeros(n)])
    c = np.cumsum(np.concatenate([[0], y]))
    return -0.691 + 10 * np.log10((c[n:] - c[:-n]).max() / n + 1e-20)


PEAK_CEIL_DB = -1.0


def soft_limit(x, ceil, look=0.002, release=0.04):
    """Smooth look-ahead gain riding (no hard clipping): the gain curve is a running
    minimum over +-look, smoothed by a Hann kernel, so it never moves faster than ~2 ms."""
    from scipy.ndimage import minimum_filter1d
    need = np.minimum(1.0, ceil * 0.995 / (np.abs(x) + 1e-12))
    w = n_of(look)
    g = minimum_filter1d(need, size=2 * w + 1)
    k = np.hanning(2 * w + 1)
    k /= k.sum()
    g = np.convolve(g, k, mode="same")
    g = np.minimum(g, minimum_filter1d(need, size=2 * w + 1))
    # release: gain recovers slowly
    a = np.exp(-1.0 / (release * SR))
    from scipy.signal import lfilter
    rel = lfilter([1 - a], [1, -a], g, zi=[g[0] * a])[0]
    g = np.minimum(g, np.maximum(rel, g))
    return x * g


def finish(x, target_lufs, lead=0.006):
    from scipy.signal import butter, sosfilt
    # remove DC / sub-rumble
    sos = butter(2, 30, "highpass", fs=SR, output="sos")
    x = sosfilt(sos, x)
    # trim tail below -62 dB of peak envelope, then a 10 ms cosine fade-out
    a = np.abs(x)
    thr = a.max() * 10 ** (-62 / 20)
    last = np.where(a > thr)[0][-1]
    x = x[: min(len(x), last + n_of(0.01))]
    nf = min(n_of(0.01), len(x) // 4)
    x[-nf:] *= raised_cos_in(nf)[::-1]
    nf_in = n_of(0.0005)
    x[:nf_in] *= raised_cos_in(nf_in)
    ceil = 10 ** (PEAK_CEIL_DB / 20)
    x0 = x.copy()
    for _ in range(4):
        x = x * 10 ** ((target_lufs - loud_s100(x)) / 20)
        if np.abs(x).max() > ceil:
            x = soft_limit(x, ceil)
    if np.abs(x).max() > ceil:
        x *= ceil / np.abs(x).max()
    # largest gain reduction vs plain scaling (for the report)
    sc = x0 * (np.abs(x).sum() / (np.abs(x0).sum() + 1e-20))
    finish.last_gr_db = float(20 * np.log10((np.abs(sc).max() + 1e-12) / (np.abs(x).max() + 1e-12)))
    x = np.concatenate([np.zeros(n_of(lead)), x, np.zeros(n_of(0.004))])
    x -= x.mean()
    return x


def write_wav(path, x):
    pcm = np.clip(np.round(x * 32767), -32768, 32767).astype("<i2")
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())


# ---------------------------------------------------------------- loudness targets
# max 100 ms K-weighted loudness (LUFS) per kind
TARGET = {
    "choice": -15.3,
    "permission": -15.3,
    "turn_done": -8.6,
    "session_change": -10.6,
    "nav": -28.4,
    "nav_edge": -14.6,
    "error": -13.0,           # between nav_edge and session_change
    "summary_failed": -16.5,  # softer than error
}


# ---------------------------------------------------------------- designs

def blip(f, dur=0.026, attack=0.0025, tau=0.0065, glide=0.0, h2=-26.0, h3=None, hold=0.003):
    parts = [(1.0, 0.0, 1.0), (2.0, h2, 0.6)]
    if h3 is not None:
        parts.append((3.0, h3, 0.5))
    return additive(dur, f, parts, attack, tau, glide_cents=glide, glide_tau=0.012, hold=hold)


def double_blip(f1, f2, gap=0.040, length=0.16, wet=-22.0, **kw):
    x = mix(length, (0.0, blip(f1, glide=-40, **kw), 0), (gap, blip(f2, **kw), 0))
    return add_space(x, wet, rt60=0.25, damp_hz=3500)


def choice(i):
    if i == 1:
        return double_blip(784.0, 1108.7)                     # G5 -> C#6, tritone up
    if i == 2:
        return double_blip(830.6, 1174.7, gap=0.042)          # G#5 -> D6
    if i == 3:
        return double_blip(740.0, 1046.5, gap=0.038, h2=-22)  # F#5 -> C6
    if i == 4:  # softer: lower, rounder, slower attack, pure
        return double_blip(659.3, 880.0, gap=0.046, attack=0.004, tau=0.009, h2=-40)
    if i == 5:  # brighter: higher with a touch of 3rd harmonic
        return double_blip(987.8, 1318.5, gap=0.038, h2=-22, h3=-28)
    if i == 6:  # shorter: tight pair
        return double_blip(784.0, 1046.5, gap=0.028, length=0.11, dur=0.018, tau=0.0045, hold=0.001)
    raise ValueError(i)


def permission(i):
    if i == 1:
        return double_blip(880.0, 1244.5, gap=0.040)          # A5 -> D#6
    if i == 2:
        return double_blip(784.0, 1046.5, gap=0.044, tau=0.008)  # G5 -> C6, a little longer
    if i == 3:
        return double_blip(740.0, 1108.7, gap=0.040)          # F#5 -> C#6, fifth
    if i == 4:  # three rising blips, more 'attention'
        x = mix(0.2, (0.0, blip(740.0, glide=-40), 0), (0.038, blip(932.3), -1),
                (0.076, blip(1108.7), -1.5))
        return add_space(x, -22.0, rt60=0.25, damp_hz=3500)
    if i == 5:  # pair played twice, second pair softer
        x = mix(0.30, (0.0, blip(784.0, glide=-40), 0), (0.040, blip(1108.7), 0),
                (0.140, blip(784.0, glide=-40), -5), (0.180, blip(1108.7), -5))
        return add_space(x, -22.0, rt60=0.25, damp_hz=3500)
    if i == 6:  # softer, lower
        return double_blip(587.3, 880.0, gap=0.048, attack=0.004, tau=0.010, h2=-40)
    raise ValueError(i)


def bell(f, dur, tau, attack=0.003, partials=None, shimmer_hz=1.3, shimmer_db=-22):
    parts = partials or [(1.0, 0.0, 1.0), (2.0, -30.0, 0.5)]
    x = additive(dur, f, parts, attack, tau)
    dur = len(x) / SR
    if shimmer_hz:  # slowly beating twin gives the long note some life
        x += 10 ** (shimmer_db / 20) * sine(dur, f + shimmer_hz) * env_perc(dur, attack, tau * 0.9)
    return x


def pip(f, dur=0.07, tau=0.014, attack=0.002):
    return additive(dur, f, [(1.0, 0.0, 1.0), (1.5, -22.0, 0.7), (2.0, -28.0, 0.5)], attack, tau, hold=0.008)


def turn_done(i):
    def two(f1, f2, gap=0.088, tau=0.17, length=1.15, wet=-18.0, p1db=0.0, **bk):
        b = bell(f2, length - gap, tau, **bk)
        tb = np.arange(len(b)) / SR
        b *= np.exp(-np.maximum(0.0, tb - 0.36) / 0.20)  # the ring closes faster near its end
        x = mix(length, (0.0, pip(f1), p1db), (gap, b, 0))
        return add_space(x, wet, rt60=0.55, damp_hz=4500)
    if i == 1:
        return two(698.5, 1318.5)                  # F5 -> E6, major seventh
    if i == 2:
        return two(740.0, 1480.0, gap=0.092)       # F#5 -> F#6, octave
    if i == 3:
        return two(659.3, 1244.5, gap=0.085, tau=0.18)  # E5 -> D#6
    if i == 4:  # softer: lower octave leap, gentler attack, no overtones
        return two(587.3, 1174.7, tau=0.17, attack=0.007, p1db=-2.0,
                   partials=[(1.0, 0.0, 1.0)], shimmer_db=-26)
    if i == 5:  # brighter: higher with a bell partial
        return two(784.0, 1568.0, partials=[(1.0, 0.0, 1.0), (2.0, -26.0, 0.5), (2.76, -24.0, 0.35)])
    if i == 6:  # shorter
        return two(698.5, 1396.9, gap=0.070, tau=0.085, length=0.6, wet=-20.0)
    raise ValueError(i)


def arpeggio(notes, onsets, last_tau=0.16, tau=0.17, length=1.15, wet=-15.0, gains=None, **ep):
    gains = gains or [0.0] * len(notes)
    ev = []
    for k, (f, at, g) in enumerate(zip(notes, onsets, gains)):
        tt = last_tau if k == len(notes) - 1 else tau
        ev.append((at, fm_epiano(length - at, f, tau=tt, **ep), g))
    x = mix(length, *ev)
    return add_space(x, wet, rt60=0.7, damp_hz=4000)


def session_change(i):
    on4 = [0.0, 0.062, 0.135, 0.200]
    if i == 1:  # E4 F#4 B4 D#5
        return arpeggio([329.6, 370.0, 493.9, 622.3], on4, gains=[-3, -6, -2, 0])
    if i == 2:  # D4 A4 C#5 E5
        return arpeggio([293.7, 440.0, 554.4, 659.3], [0.0, 0.066, 0.132, 0.205], gains=[-3, -4, -2, 0])
    if i == 3:  # G4 A4 D5 F#5
        return arpeggio([392.0, 440.0, 587.3, 740.0], on4, gains=[-3, -6, -2, 0], index=0.45)
    if i == 4:  # softer: three notes, pure tone, no tines
        return arpeggio([349.2, 440.0, 523.3], [0.0, 0.085, 0.170], index=0.25, tine=(),
                        attack=0.008, gains=[-3, -2, 0], wet=-14.0)
    if i == 5:  # brighter: higher register, stronger tines
        return arpeggio([440.0, 554.4, 659.3, 830.6], on4, gains=[-3, -5, -2, 0], index=1.2,
                        tine=((13.6, -14, 0.014), (17.3, -19, 0.009)))
    if i == 6:  # shorter: quick three-note run, short ring
        return arpeggio([392.0, 493.9, 587.3], [0.0, 0.050, 0.100], last_tau=0.08, tau=0.07,
                        length=0.5, gains=[-3, -2, 0], wet=-18.0)
    raise ValueError(i)


def nav(i):
    if i == 1:
        return tick(1900.0, 640.0, 0.0042)
    if i == 2:
        return tick(1720.0, 575.0, 0.0050, lo_db=-7)
    if i == 3:
        return tick(2080.0, 700.0, 0.0036)
    if i == 4:  # softer, woodier
        return tick(1150.0, 560.0, 0.0065, lo_db=-4)
    if i == 5:  # brighter
        return tick(2800.0, 930.0, 0.0030, lo_db=-12)
    if i == 6:  # lightest single partial
        return tick(2400.0, 0.0, 0.0026)
    raise ValueError(i)


EDGE_MODES = [(0.61, -12.0, 1.1), (1.0, 0.0, 1.0), (1.9, -16.0, 0.7), (2.97, -19.0, 0.55), (3.68, -22.0, 0.45)]


def edge(f0, length=0.5, wet=-20.0, **kw):
    kw.setdefault("tail_tau_mult", 2.0)
    x = modal(length, f0, kw.pop("modes", EDGE_MODES), **kw)
    return add_space(x, wet, rt60=0.3, damp_hz=3000)


def nav_edge(i):
    if i == 1:
        return edge(240.0, beat_hz=2.6, tau=0.075)
    if i == 2:
        return edge(262.0, beat_hz=3.4, tau=0.07)
    if i == 3:
        return edge(225.0, beat_hz=2.0, tau=0.065,
                    modes=[(0.6, -9.0, 1.2), (1.0, 0.0, 1.0), (1.88, -15.0, 0.7), (3.02, -18.0, 0.5)])
    if i == 4:  # softer: rounder, fewer upper modes, slower attack
        return edge(205.0, beat_hz=2.2, attack=0.004,
                    modes=[(0.6, -12.0, 1.1), (1.0, 0.0, 1.0), (1.9, -24.0, 0.6)])
    if i == 5:  # brighter knock
        return edge(285.0, beat_hz=3.0, modes=[(0.61, -14.0, 1.0), (1.0, 0.0, 1.0), (1.93, -11.0, 0.7),
                                                 (3.0, -13.0, 0.5), (4.1, -18.0, 0.35)])
    if i == 6:  # shorter
        return edge(240.0, length=0.22, tau=0.03, tail_db=-30, beat_hz=0.0, wet=-24.0)
    raise ValueError(i)


def error(i):
    if i == 1:  # soft bell pair falling a minor third
        x = mix(0.7, (0.0, bell(440.0, 0.25, 0.07, shimmer_db=-30), 0),
                (0.115, bell(370.0, 0.58, 0.16, shimmer_db=-30), 0))
        return add_space(x, -17.0, rt60=0.5)
    if i == 2:  # e-piano falling major third (session family)
        x = mix(0.8, (0.0, fm_epiano(0.8, 329.6, tau=0.09), -2), (0.12, fm_epiano(0.68, 261.6, tau=0.17), 0))
        return add_space(x, -16.0, rt60=0.6)
    if i == 3:  # low bonk pair falling (nav_edge family)
        x = mix(0.5, (0.0, modal(0.5, 300.0, EDGE_MODES, tau=0.05, beat_hz=2.5), -2),
                (0.11, modal(0.39, 238.0, EDGE_MODES, tau=0.07, beat_hz=2.0), 0))
        return add_space(x, -19.0, rt60=0.35, damp_hz=3000)
    if i == 4:  # single tone sagging two semitones
        x = additive(0.6, 392.0, [(1.0, 0.0, 1.0), (2.0, -24.0, 0.5)], 0.004, 0.16,
                     glide_cents=200, glide_tau=0.06)
        return add_space(x, -17.0, rt60=0.5)
    if i == 5:  # three-note descent G4 E4 C4
        x = mix(0.8, (0.0, bell(392.0, 0.3, 0.06, shimmer_db=-32), -3),
                (0.09, bell(329.6, 0.3, 0.06, shimmer_db=-32), -2),
                (0.18, bell(261.6, 0.62, 0.16, shimmer_db=-32), 0))
        return add_space(x, -17.0, rt60=0.5)
    if i == 6:  # low soft falling fourth, bonk timbre, shorter
        x = mix(0.4, (0.0, modal(0.4, 293.7, EDGE_MODES[:3], tau=0.045, attack=0.003), -2),
                (0.10, modal(0.3, 220.0, EDGE_MODES[:3], tau=0.06, attack=0.003), 0))
        return add_space(x, -20.0, rt60=0.3, damp_hz=3000)
    raise ValueError(i)


def summary_failed(i):
    pure = [(1.0, 0.0, 1.0)]
    if i == 1:  # slow soft falling fourth
        x = mix(0.8, (0.0, additive(0.3, 523.3, pure, 0.012, 0.08), -1),
                (0.14, additive(0.66, 392.0, pure, 0.014, 0.17), 0))
        return add_space(x, -15.0, rt60=0.6)
    if i == 2:  # gentle e-piano D4 -> B3
        x = mix(0.8, (0.0, fm_epiano(0.8, 293.7, tau=0.09, index=0.5, tine=()), -2),
                (0.13, fm_epiano(0.67, 246.9, tau=0.17, index=0.5, tine=()), 0))
        return add_space(x, -15.0, rt60=0.6)
    if i == 3:  # one soft tone sinking a semitone
        x = additive(0.65, 440.0, pure, 0.015, 0.17, glide_cents=100, glide_tau=0.09)
        return add_space(x, -15.0, rt60=0.6)
    if i == 4:  # two muted low notes
        x = mix(0.6, (0.0, additive(0.25, 349.2, pure + [(2.0, -34, 0.5)], 0.010, 0.06), -1),
                (0.12, additive(0.48, 293.7, pure + [(2.0, -34, 0.5)], 0.010, 0.12), 0))
        return add_space(x, -18.0, rt60=0.4)
    if i == 5:  # three quiet notes falling A4 F4 D4
        x = mix(0.85, (0.0, additive(0.3, 440.0, pure, 0.008, 0.05), -3),
                (0.10, additive(0.3, 349.2, pure, 0.008, 0.05), -2),
                (0.20, additive(0.65, 293.7, pure, 0.010, 0.15), 0))
        return add_space(x, -16.0, rt60=0.55)
    if i == 6:  # short soft single 'deflate'
        x = additive(0.32, 370.0, pure, 0.010, 0.07, glide_cents=150, glide_tau=0.05)
        return add_space(x, -18.0, rt60=0.35)
    raise ValueError(i)


EVENTS = {
    "choice": choice,
    "permission": permission,
    "turn_done": turn_done,
    "session_change": session_change,
    "nav": nav,
    "nav_edge": nav_edge,
    "error": error,
    "summary_failed": summary_failed,
}

DESCR = {
    "choice": ["two-blip rise G5-C#6", "two-blip rise G#5-D6", "two-blip rise F#5-C6, rounder",
               "softer, lower E5-A5", "brighter B5-E6", "shorter, tight pair"],
    "permission": ["two-blip rise A5-D#6", "two-blip rise G5-C6, longer", "two-blip rise a fifth",
                   "three rising blips", "pair played twice", "softer, lower D5-A5"],
    "turn_done": ["pip F5 then bell E6 (maj 7th)", "pip F#5 then bell F#6 (octave)", "pip E5 then bell D#6",
                  "softer, lower octave leap", "brighter, bell partial", "shorter ring"],
    "session_change": ["e-piano arp E4 F#4 B4 D#5", "e-piano arp D4 A4 C#5 E5", "e-piano arp G4 A4 D5 F#5",
                       "softer, 3 pure notes", "brighter, higher arp", "shorter, quick 3-note run"],
    "nav": ["tick 1.9k", "tick 1.7k, fuller", "tick 2.1k, tighter", "softer, woody", "brighter 2.8k",
            "lightest single-partial"],
    "nav_edge": ["low bonk 240 Hz", "low bonk 262 Hz", "low bonk 225 Hz, deeper", "softer, rounder",
                 "brighter knock", "shorter"],
    "error": ["bell pair falling minor 3rd", "e-piano falling major 3rd", "bonk pair falling",
              "single tone sagging 2 semitones", "three-note descent", "low bonk falling 4th, short"],
    "summary_failed": ["soft falling 4th", "soft e-piano D4-B3", "one soft tone sinking", "two muted low notes",
                       "three quiet notes falling", "short soft deflate"],
}


GR = {}


def render_all(out_dir):
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    paths = {}
    for ev, fn in EVENTS.items():
        for i in range(1, 7):
            x = finish(fn(i), TARGET[ev])
            GR[(ev, i)] = finish.last_gr_db
            p = out / f"{ev}-{i}.wav"
            write_wav(p, x)
            paths[(ev, i)] = p
    return paths


if __name__ == "__main__":
    here = Path(__file__).parent
    out_dir = sys.argv[1] if len(sys.argv) > 1 and not sys.argv[1].startswith("--") else str(here / "out")
    render_all(out_dir)
