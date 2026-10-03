"""Sonara's earcon candidates, round 2: question, reply_finished, session_changed.

Original work, Sonara, MIT licence. Pure procedural synthesis (numpy + scipy),
no samples. Voices are rendered at 4x (192 kHz), low-passed at 16 kHz and
decimated to 48 kHz with a polyphase anti-alias filter; then an optional
seeded noise-tail room (make_pack.add_space), DC block, tail trim, fades,
loudness match on the s100 scale (make_pack.loud_s100) with smooth look-ahead
limiting under a -3.2 dBFS peak ceiling. Output: 48 kHz, 16-bit, mono WAV.

    python make_round2.py [outdir]        (default: ./out)
"""
from __future__ import annotations

import json
import os
import sys

import numpy as np
from scipy import signal

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import make_pack as mp  # noqa: E402     (room, s100 loudness, soft limiter)
import synth as ml  # noqa: E402  (192 kHz primitives)

FS = ml.FS          # 192 kHz render rate
SR = ml.SR          # 48 kHz output
OS = ml.OS
note = ml.note
tvec = ml.tvec
n_of = ml.n_of

TARGET = {"question": -15.0, "reply_finished": -10.0, "session_changed": -11.0}
DUR = {"question": (0.12, 0.35), "reply_finished": (0.30, 0.75), "session_changed": (0.30, 1.00)}
CEIL_DB = -3.2


# ------------------------------------------------------------------ helpers

def tidy(x, fout=0.004):
    """Close a voice buffer with a short raised-cosine fade so a cut never clicks."""
    x = np.array(x, dtype=float)
    nf = min(n_of(fout), len(x) // 4)
    x[-nf:] *= 0.5 + 0.5 * np.cos(np.linspace(0, np.pi, nf))
    return x


def smx(*xs):
    """Sum voices of different lengths; each is closed with a short fade first."""
    n = max(len(x) for x in xs)
    out = np.zeros(n)
    for x in xs:
        out[: len(x)] += tidy(x)
    return out


def lay(*events):
    """events: (onset_s, signal, gain_db). The buffer grows to fit every event."""
    total = max(n_of(at) + len(x) for at, x, _ in events)
    out = np.zeros(total)
    for at, x, g in events:
        i = n_of(at) if at > 0 else 0
        out[i:i + len(x)] += 10 ** (g / 20) * tidy(x)
    return out


def env2(dur, attack, tau, hold=0.0, tail_db=None, tail_tau=None):
    """Raised-cosine attack, exponential decay; optional quieter long second stage."""
    e = ml.env(dur, attack, tau, hold)
    if tail_db is not None:
        e = e + 10 ** (tail_db / 20) * ml.env(dur, attack * 2, tail_tau, hold)
        e /= e.max()
    return e


def pitch_curve(f, dur, start_st=0.0, tau=0.015, end_st=0.0, end_at=None, end_tau=0.03):
    """Frequency track: start_st semitones away settling to f; optional late bend by end_st."""
    t = tvec(dur)
    st = start_st * np.exp(-t / tau)
    if end_st:
        u = np.clip((t - end_at) / end_tau, 0, 1)
        st = st + end_st * (0.5 - 0.5 * np.cos(np.pi * u))
    return f * 2 ** (st / 12)


def sine_track(freq):
    return np.sin(2 * np.pi * np.cumsum(freq) / FS)


# ------------------------------------------------------------------ voices (192 kHz)

def v_drop(f, dur=0.11, fall_st=9.0, fall_tau=0.010, tau=0.030, bright=0.06):
    """Pitched water drop: starts high and falls fast onto f ('dop')."""
    fr = pitch_curve(f, dur, fall_st, fall_tau)
    x = ml.osc(fr, dur, ((1, 1.0), (2, bright)))
    return x * ml.env(dur, 0.0012, tau)


def v_bubble(f, dur=0.09, rise_st=7.0, tau=0.024):
    """Bubble 'bloop': rises onto f (Minnaert-style upward chirp), very short."""
    fr = pitch_curve(f, dur, -rise_st, 0.016)
    return ml.osc(fr, dur, ((1, 1.0), (2, 0.04))) * ml.env(dur, 0.002, tau)


def v_marimba(f, dur=0.25, tau=None, mallet=0.06):
    """Rosewood bar, tuned 1:4:10 with a resonator tube on the fundamental, soft yarn mallet."""
    tau = tau or 0.11 * (440.0 / f) ** 0.4
    bar = ml.modal(f, [1, 4.0, 9.9], [1.0, 0.16, 0.035], [tau, tau * 0.22, tau * 0.07], dur, attack=0.0012)
    tube = ml.osc(f, dur) * env2(dur, 0.004, tau * 1.5)
    knock = ml.lowpass(ml.burst(min(dur, 0.012), f * 1.6, 0.7, 0.0025), 2500)
    return smx(ml.unit(bar), 0.35 * ml.unit(tube), mallet * ml.unit(knock))


def v_glass(f, dur=0.25, tau=0.07):
    """Crystal ting: inharmonic modes 1:2.61:4.83 plus a slowly beating twin."""
    x = ml.modal(f, [1, 2.61, 4.83], [1.0, 0.22, 0.07], [tau, tau * 0.4, tau * 0.2], dur, attack=0.0008)
    x += 0.25 * ml.osc(f * 1.0012, dur) * ml.env(dur, 0.0008, tau * 0.9)
    return x


def v_felt(f, dur=0.35, tau=0.18, felt=2200.0):
    """Felt piano: stretched harmonics, three detuned strings, soft hammer thump."""
    B = 0.00035
    out = np.zeros(n_of(dur))
    for k in range(1, 9):
        fk = k * f * np.sqrt(1 + B * k * k)
        if fk > 15000:
            break
        a = (1 / k ** 1.6) * np.exp(-fk / felt)
        tk = tau / (1 + 0.45 * (k - 1))
        for d in (-0.35, 0.0, 0.4):
            out += a / 3 * ml.osc(fk + d, dur, phase0=k * 0.7 + d) * env2(dur, 0.004, tk, tail_db=-12, tail_tau=tk * 2.5)
    thump = ml.lowpass(ml.burst(min(dur, 0.03), 260, 0.7, 0.007), 700)
    return smx(ml.unit(out), 0.05 * ml.unit(thump))


def v_kalimba(f, dur=0.3, tau=0.12):
    """Thumb-piano tine: 1:5.95:14.2 with fast-dying upper modes and a soft thumb contact."""
    x = ml.modal(f, [1, 5.95, 14.2], [1.0, 0.16, 0.04], [tau, tau * 0.09, tau * 0.04], dur, attack=0.0007)
    x += 0.08 * ml.osc(2 * f, dur) * ml.env(dur, 0.001, tau * 0.3)
    thumb = ml.lowpass(ml.burst(min(dur, 0.01), 900, 0.8, 0.0015), 2000)
    return smx(ml.unit(x), 0.05 * ml.unit(thumb))


def v_round(f, dur=0.14, tau=0.05, start_st=-0.5, hold=0.012, end_st=0.0, attack=0.006):
    """Rounded sine 'doo'/'dee' with a small scoop into pitch, optional late upward bend."""
    fr = pitch_curve(f, dur, start_st, 0.02, end_st, end_at=0.03, end_tau=0.06)
    return ml.osc(fr, dur, ((1, 1.0), (2, 0.05), (3, 0.012))) * ml.env(dur, attack, tau, hold)


def v_pluck(f, dur=0.35, tau=0.12, damp=3200.0, exc_lp=3500.0, seed=0):
    """Karplus-Strong string at 192 kHz: fractional delay + one-pole damping in the loop.

    Rendered block-wise: a block shorter than the loop delay depends only on earlier
    output, so every block is computed with vector ops (one-pole state carried)."""
    rng = np.random.default_rng(1000 + seed)
    n = n_of(dur)
    # the loop must lose energy at every frequency: the damping corner has to sit high
    # enough that |H(f)| >= the per-period decay wanted at f, and g stays below 1
    damp = min(max(damp, 1.15 * f * np.sqrt(f * tau / 2)), 30000.0)
    p = np.exp(-2 * np.pi * damp / FS)
    w = 2 * np.pi * f / FS
    pd = np.arctan2(p * np.sin(w), 1 - p * np.cos(w)) / w
    D = FS / f - pd
    mag = (1 - p) / np.abs(1 - p * np.exp(-1j * w))
    g = min(np.exp(-1.0 / (f * tau)) / mag, 0.9995)
    P = int(D)
    exc = rng.standard_normal(P)
    exc = signal.sosfiltfilt(signal.butter(2, exc_lp, "lowpass", fs=FS, output="sos"), exc)
    exc -= exc.mean()
    exc *= signal.windows.tukey(P, 0.25)
    y = np.zeros(n + P + 2)
    y[:P] = exc
    pos = P
    zi = np.zeros(1)
    blk = max(1, P - 2)
    b, a = [1 - p], [1, -p]
    while pos < len(y):
        m = min(blk, len(y) - pos)
        idx = np.arange(pos, pos + m) - D
        i0 = np.floor(idx).astype(int)
        fr = idx - i0
        d = (1 - fr) * y[i0] + fr * y[i0 + 1]
        lp, zi = signal.lfilter(b, a, d, zi=zi)
        y[pos:pos + m] = g * lp
        pos += m
    y = y[:n]
    y = ml.lowpass(y, 7000, 4)
    return y * ml.env(dur, 0.0006, 10.0) * np.minimum(1, np.linspace(1, 0, n) * 20)


def v_bell(f, dur=0.4, tau=0.15):
    """Soft bell, major-third bell partials (hum 0.5, tierce 1.26, quint 1.5, nominal 2)."""
    return ml.modal(f, [0.5, 1, 1.26, 1.5, 2.0, 2.66],
                    [0.18, 1.0, 0.16, 0.12, 0.22, 0.05],
                    [tau * 1.5, tau, tau * 0.55, tau * 0.5, tau * 0.4, tau * 0.25], dur, attack=0.0015)


def v_wood(f, dur=0.09, tau=0.028):
    """Soft woodblock knock: short modes 1:2.32:4.1, felt-covered strike."""
    x = ml.modal(f, [1, 2.32, 4.1], [1.0, 0.25, 0.07], [tau, tau * 0.4, tau * 0.2], dur, attack=0.001)
    strike = ml.lowpass(ml.burst(min(dur, 0.008), f * 2, 0.8, 0.0012), 3000)
    return smx(ml.unit(x), 0.1 * ml.unit(strike))


def v_vibe(f, dur=0.4, tau=0.2, trem=5.2, depth=0.22):
    """Vibraphone-ish aluminium bar (1:3.98:9.4) with a gentle motor tremolo."""
    x = ml.modal(f, [1, 3.98, 9.4], [1.0, 0.12, 0.03], [tau, tau * 0.15, tau * 0.05], dur, attack=0.0012)
    t = tvec(dur)
    return x * (1 - depth * (0.5 - 0.5 * np.cos(2 * np.pi * trem * t)))


def v_ding(f, dur=0.6, tau=0.2, attack=0.002, shimmer=1.1, tail_db=-14, tail_tau=None):
    """Pure sine ding with a beating twin and a quiet long second-stage tail."""
    tail_tau = tail_tau or tau * 2.2
    e = env2(dur, attack, tau, tail_db=tail_db, tail_tau=tail_tau)
    x = ml.osc(f, dur, ((1, 1.0), (2, 0.025))) * e
    x += 0.12 * ml.osc(f + shimmer, dur) * e
    return x


def v_pip(f, dur=0.07, tau=0.014):
    return ml.osc(f, dur, ((1, 1.0), (2, 0.05))) * ml.env(dur, 0.0018, tau, hold=0.006)


def v_epiano(f, dur=0.6, tau=0.25, index=0.7, index_tau=0.05, tine_ratio=7.0, tine_db=-24):
    """Two FM pairs: a 1:1 body with decaying index and a short 1:7 tine (all well under 16 kHz)."""
    t = tvec(dur)
    I = index * np.exp(-t / index_tau) + 0.06
    body = np.sin(2 * np.pi * f * t + I * np.sin(2 * np.pi * f * t)) * env2(dur, 0.003, tau, tail_db=-10, tail_tau=tau * 2)
    ft = f * tine_ratio
    while ft > 9000:
        ft /= 2
    tine = np.sin(2 * np.pi * ft * t + 0.8 * np.exp(-t / 0.01) * np.sin(2 * np.pi * f * t)) * ml.env(dur, 0.0012, 0.018)
    return body + 10 ** (tine_db / 20) * tine


def v_pad(freqs, dur=0.6, attack=0.06, tau=0.3, detune=0.0018, hold=0.04):
    out = np.zeros(n_of(dur))
    for f in freqs:
        for d in (-detune, detune):
            out += ml.osc(f * (1 + d), dur, ((1, 1.0), (2, 0.08), (3, 0.02)),
                          phase0=float(np.random.default_rng(int(f * 10) + (d > 0)).uniform(0, 2 * np.pi)))
    out = ml.lowpass(out, 3000, 2)
    return out * ml.env(dur, attack, tau, hold)


def v_air(dur, f0, f1, q=1.4, curve=0.6):
    """Airy band-passed noise sweep (page-turn / swoosh), soft edges."""
    u = np.linspace(0, 1, n_of(dur))
    shape = np.sin(np.pi * u ** curve) ** 2
    return ml.lowpass(ml.sweep_noise(dur, f0, f1, q, shape=shape), 7000, 4)


def v_sparkle(dur, n=10, lo=2500.0, hi=7000.0, rise=True, seed=0):
    """Short high sine grains at seeded random times; pitch trend rises across the cloud."""
    rng = np.random.default_rng(500 + seed)
    out = np.zeros(n_of(dur + 0.1))
    for i in range(n):
        u = (i + rng.uniform(0, 0.8)) / n
        f = lo * (hi / lo) ** (u if rise else 1 - u) * rng.uniform(0.96, 1.04)
        g = rng.uniform(0.5, 1.0)
        x = ml.osc(f, 0.09) * ml.env(0.09, 0.001, 0.02)
        at = n_of(u * dur)
        out[at:at + len(x)] += g * x
    return out


def chord(voice, freqs, strum, gains=None):
    gains = gains or [0.0] * len(freqs)
    return lay(*[(i * strum, voice(f), g) for i, (f, g) in enumerate(zip(freqs, gains))])


N = note


# ------------------------------------------------------------------ question (24)
# all two-note, 40-140 ms apart, short tails, total <= 350 ms

def two(v1, v2, gap, g2=0.0, g1=0.0):
    return lay((0.0, v1, g1), (gap, v2, g2))


QUESTION = [
    ("water drops, same pitch double 'dop-dop'",
     lambda: two(v_drop(N("A5")), v_drop(N("A5")), 0.095, g2=0.5), None),
    ("water drops, rising fourth",
     lambda: two(v_drop(N("G5")), v_drop(N("C6")), 0.100), None),
    ("water drops, rising fifth, second drop louder",
     lambda: two(v_drop(N("D5"), tau=0.034), v_drop(N("A5"), tau=0.034), 0.110, g2=2.5, g1=-1.0), None),
    ("bubbles 'bloop-bloop', rising major third",
     lambda: two(v_bubble(N("E5")), v_bubble(N("G#5")), 0.090), None),
    ("soft marimba 'da-da', rising major third (low)",
     lambda: two(v_marimba(N("C5"), 0.24), v_marimba(N("E5"), 0.24), 0.115, g2=0.5), (-26, 0.18)),
    ("soft marimba 'da-da', rising fifth",
     lambda: two(v_marimba(N("G4"), 0.24), v_marimba(N("D5"), 0.24), 0.125), (-26, 0.18)),
    ("soft marimba same-pitch double tap, second louder",
     lambda: two(v_marimba(N("A5"), 0.2), v_marimba(N("A5"), 0.22), 0.080, g2=2.0, g1=-1.0), (-26, 0.18)),
    ("glass 'ting-ting', rising fourth",
     lambda: two(v_glass(N("E6"), 0.2, 0.05), v_glass(N("A6"), 0.22, 0.06), 0.090, g2=-1.0), (-24, 0.2)),
    ("glass same-pitch double 'ting-ting', second louder",
     lambda: two(v_glass(N("B6"), 0.18, 0.045), v_glass(N("B6"), 0.22, 0.06), 0.070, g2=2.0, g1=-2.0), (-24, 0.2)),
    ("felt piano two notes, rising major sixth",
     lambda: two(v_felt(N("G4"), 0.25, 0.09), v_felt(N("E5"), 0.22, 0.09), 0.130), (-24, 0.22)),
    ("felt piano two notes, rising fourth",
     lambda: two(v_felt(N("D5"), 0.22, 0.08), v_felt(N("G5"), 0.22, 0.08), 0.115, g2=1.0), (-24, 0.22)),
    ("kalimba 'da-di', rising fifth",
     lambda: two(v_kalimba(N("D5"), 0.22, 0.07), v_kalimba(N("A5"), 0.22, 0.07), 0.100), (-26, 0.2)),
    ("kalimba, rising octave",
     lambda: two(v_kalimba(N("G4"), 0.22, 0.08), v_kalimba(N("G5"), 0.22, 0.07), 0.115), (-26, 0.2)),
    ("rounded sine 'doo-dee', rising major third with a scoop",
     lambda: two(v_round(N("C#5")), v_round(N("F5"), start_st=-0.8), 0.110), None),
    ("rounded sine 'doo-dee', rising fourth, second note bends up (asking)",
     lambda: two(v_round(N("A4"), 0.13, 0.045), v_round(N("D5"), 0.16, 0.06, end_st=0.6), 0.120, g2=1.0), None),
    ("Karplus-Strong pluck, rising fifth",
     lambda: two(v_pluck(N("A4"), 0.2, 0.06, seed=1), v_pluck(N("E5"), 0.22, 0.06, seed=2), 0.100), (-26, 0.18)),
    ("Karplus-Strong pluck, same-pitch double, second louder",
     lambda: two(v_pluck(N("D5"), 0.18, 0.05, seed=3), v_pluck(N("D5"), 0.22, 0.06, seed=4), 0.075, g2=2.0, g1=-1.0), (-26, 0.18)),
    ("soft bell two notes, rising major third",
     lambda: two(v_bell(N("F5"), 0.2, 0.06), v_bell(N("A5"), 0.22, 0.07), 0.120), (-24, 0.2)),
    ("soft woodblock knock pair, rising fourth",
     lambda: two(v_wood(N("B5")), v_wood(N("E6")), 0.080), (-28, 0.15)),
    ("soft woodblock same-pitch double knock 'tok-tok'",
     lambda: two(v_wood(N("F#5"), tau=0.03), v_wood(N("F#5"), tau=0.03), 0.065, g2=1.0), (-28, 0.15)),
    ("vibraphone mallet, rising octave",
     lambda: two(v_vibe(N("C5"), 0.22, 0.08, depth=0.0), v_vibe(N("C6"), 0.22, 0.07, depth=0.0), 0.125, g2=-1.0), (-24, 0.2)),
    ("vibraphone mallet, rising fourth, second louder",
     lambda: two(v_vibe(N("E5"), 0.2, 0.07, depth=0.0), v_vibe(N("A5"), 0.22, 0.08, depth=0.0), 0.105, g2=2.0, g1=-1.0), (-24, 0.2)),
    ("water drop then glass ting, rising octave",
     lambda: two(v_drop(N("E5"), tau=0.03), v_glass(N("E6"), 0.2, 0.05), 0.105, g2=-2.0), (-26, 0.18)),
    ("marimba + sine layer 'da-DEE', rising major sixth, second louder",
     lambda: two(smx(v_marimba(N("D5"), 0.2), 0.5 * v_round(N("D5"), 0.12, 0.04)),
                 smx(v_marimba(N("B5"), 0.22), 0.5 * v_round(N("B5"), 0.14, 0.05)), 0.110, g2=2.0, g1=-1.0), (-26, 0.18)),
]


# ------------------------------------------------------------------ reply_finished (20)

def pipding(pipv, dingv, gap, gp=-1.5):
    return lay((0.0, pipv, gp), (gap, dingv, 0.0))


REPLY = [
    ("pip then sine ding, major sixth up (E5 -> C#6)",
     lambda: pipding(v_pip(N("E5")), v_ding(N("C#6"), 0.6, 0.15), 0.090), (-20, 0.5)),
    ("felt pip then soft ding, fifth up (A5 -> E6)",
     lambda: pipding(v_felt(N("A5"), 0.12, 0.03), v_ding(N("E6"), 0.6, 0.14, attack=0.004), 0.085), (-20, 0.5)),
    ("marimba pip then glass-sine ding, octave (D5 -> D6)",
     lambda: pipding(v_marimba(N("D5"), 0.15, 0.05), smx(v_ding(N("D6"), 0.6, 0.15), 0.35 * v_glass(N("D6"), 0.3, 0.08)), 0.095), (-20, 0.5)),
    ("kalimba pip then bell ding, major seventh (Eb5 -> D6)",
     lambda: pipding(v_kalimba(N("Eb5"), 0.15, 0.04), v_bell(N("D6"), 0.6, 0.17), 0.090, gp=-3), (-20, 0.5)),
    ("single soft bell, long quiet tail (A5)",
     lambda: v_bell(N("A5"), 0.7, 0.22) * ml.env(0.7, 0.0, 0.6), (-18, 0.6)),
    ("felt piano falling fifth resolve (E5 -> A4)",
     lambda: lay((0, v_felt(N("E5"), 0.2, 0.08), -2), (0.12, v_felt(N("A4"), 0.6, 0.22), 0)), (-18, 0.55)),
    ("glass falling fifth resolve, soft (D6 -> G5)",
     lambda: lay((0, v_glass(N("D6"), 0.2, 0.06), -3), (0.11, v_glass(N("G5"), 0.6, 0.2), 0)), (-18, 0.55)),
    ("kalimba three-note arpeggio landing on the tonic (B4 D5 G5)",
     lambda: lay((0, v_kalimba(N("B4"), 0.2, 0.07), -4), (0.075, v_kalimba(N("D5"), 0.2, 0.07), -3),
                 (0.15, v_kalimba(N("G5"), 0.55, 0.2), 0)), (-19, 0.5)),
    ("vibraphone root-fifth-octave, tremolo tail (A4 E5 A5)",
     lambda: lay((0, v_vibe(N("A4"), 0.2, 0.08, depth=0.1), -4), (0.07, v_vibe(N("E5"), 0.2, 0.08, depth=0.1), -3),
                 (0.14, v_vibe(N("A5"), 0.6, 0.22), 0)), (-18, 0.55)),
    ("glass ting over a soft G major pad bloom",
     lambda: lay((0, v_pad([N("G4"), N("B4"), N("D5")], 0.6, 0.04, 0.16), -6), (0.03, v_glass(N("G6"), 0.55, 0.15), -1)), (-17, 0.6)),
    ("marimba flam on a major sixth (E5 + C#6), ringing",
     lambda: lay((0, v_marimba(N("E5"), 0.5, 0.17), -1), (0.022, v_marimba(N("C#6"), 0.5, 0.17), 0)), (-19, 0.5)),
    ("kalimba falling resolve D6 A5 D5",
     lambda: lay((0, v_kalimba(N("D6"), 0.2, 0.06), -5), (0.08, v_kalimba(N("A5"), 0.2, 0.07), -3),
                 (0.16, v_kalimba(N("D5"), 0.55, 0.22), 0)), (-19, 0.5)),
    ("success chord bloom, soft Cmaj9 pad with glass top",
     lambda: lay((0, v_pad([N("C4"), N("E4"), N("G4"), N("B4"), N("D5")], 0.65, 0.05, 0.17), -2),
                 (0.05, v_glass(N("E6"), 0.5, 0.13), -9)), (-16, 0.6)),
    ("strummed felt piano D major chord bloom",
     lambda: chord(lambda f: v_felt(f, 0.6, 0.2), [N("D4"), N("F#4"), N("A4"), N("D5")], 0.022, [-4, -4, -3, 0]), (-18, 0.55)),
    ("felt piano rising octave, soft una corda (F5 -> F6)",
     lambda: pipding(v_felt(N("F5"), 0.14, 0.04, felt=1600), v_felt(N("F6"), 0.6, 0.18, felt=1800), 0.090, gp=-1), (-19, 0.5)),
    ("pip then stretched-bell ding, major sixth (G5 -> E6), roomy",
     lambda: pipding(v_pip(N("G5")), v_bell(N("E6"), 0.6, 0.17), 0.090), (-16, 0.6)),
    ("pluck then sine bell, fifth up (C5 -> G5) with octave shimmer",
     lambda: pipding(v_pluck(N("C5"), 0.15, 0.04, seed=11),
                     smx(v_ding(N("G5"), 0.6, 0.17), 0.18 * v_ding(N("G6"), 0.4, 0.08)), 0.095, gp=-1), (-19, 0.5)),
    ("rolled felt piano G4 D5 G5, landing on the octave",
     lambda: lay((0, v_felt(N("G4"), 0.6, 0.18), -5), (0.06, v_felt(N("D5"), 0.6, 0.18), -4),
                 (0.12, v_felt(N("G5"), 0.6, 0.22), 0)), (-18, 0.55)),
    ("rounded sine falling major third resolve (C#6 -> A5)",
     lambda: lay((0, v_round(N("C#6"), 0.12, 0.04), -2), (0.10, v_ding(N("A5"), 0.6, 0.16, attack=0.005), 0)), (-18, 0.55)),
    ("pip-pip-ding: two quick pips then the octave (E5 E5 -> E6)",
     lambda: lay((0, v_pip(N("E5"), 0.05, 0.010), -3), (0.06, v_pip(N("E5"), 0.05, 0.010), -3),
                 (0.13, v_ding(N("E6"), 0.6, 0.15), 0)), (-20, 0.5)),
]


# ------------------------------------------------------------------ session_changed (20)

def arp(voice, freqs, step, gains=None, start=0.0):
    gains = gains or [-3.0] * (len(freqs) - 1) + [0.0]
    return [(start + i * step, voice(f), g) for i, (f, g) in enumerate(zip(freqs, gains))]


SESSION = [
    ("e-piano arpeggio Bb3 F4 A4 D5, slow and warm",
     lambda: lay(*arp(lambda f: v_epiano(f, 0.75, 0.25), [N("Bb3"), N("F4"), N("A4"), N("D5")], 0.08, [-4, -4, -3, 0])), (-15, 0.7)),
    ("glass arpeggio A5 C#6 E6 A6",
     lambda: lay(*arp(lambda f: v_glass(f, 0.6, 0.18), [N("A5"), N("C#6"), N("E6"), N("A6")], 0.055, [-3, -4, -4, -2])), (-16, 0.7)),
    ("harp-like pluck run D4 A4 D5 F#5 A5",
     lambda: lay(*arp(lambda f: v_pluck(f, 0.7, 0.25, damp=4500, exc_lp=2600, seed=int(f)), [N("D4"), N("A4"), N("D5"), N("F#5"), N("A5")], 0.045,
                      [-5, -5, -4, -3, 0])), (-16, 0.7)),
    ("soft swoosh rising into a glass chime (E6)",
     lambda: lay((0, v_air(0.3, 600, 5000, 1.6), -6), (0.24, v_glass(N("E6"), 0.6, 0.2), 0), (0.24, v_ding(N("E5"), 0.5, 0.15), -10)), (-16, 0.7)),
    ("soft swoosh falling onto a kalimba landing (A4 E5)",
     lambda: lay((0, v_air(0.28, 5000, 900, 1.6, curve=0.4), -6), (0.2, v_kalimba(N("A4"), 0.5, 0.18), -2),
                 (0.26, v_kalimba(N("E5"), 0.55, 0.2), 0)), (-17, 0.6)),
    ("two-chord shift, felt piano C major -> D major",
     lambda: lay((0, chord(lambda f: v_felt(f, 0.3, 0.1), [N("C4"), N("E4"), N("G4"), N("C5")], 0.015), -2),
                 (0.17, chord(lambda f: v_felt(f, 0.65, 0.22), [N("D4"), N("F#4"), N("A4"), N("D5")], 0.015), 0)), (-17, 0.6)),
    ("two-chord shift, soft pads Fmaj7 -> G6 swell",
     lambda: lay((0, v_pad([N("F4"), N("A4"), N("C5"), N("E5")], 0.35, 0.05, 0.1), -1),
                 (0.2, v_pad([N("G4"), N("B4"), N("D5"), N("E5")], 0.7, 0.06, 0.2), 0)), (-17, 0.6)),
    ("sideways glide A4 -> E5 landing on a sine bell",
     lambda: lay((0, sine_track(pitch_curve(N("E5"), 0.24, -7, 0.06)) * ml.env(0.24, 0.02, 0.09, 0.08), -3),
                 (0.17, v_ding(N("E5"), 0.6, 0.18), 0), (0.17, v_glass(N("E6"), 0.4, 0.1), -12)), (-17, 0.6)),
    ("swoop: glide dips then lands on A5 with a mallet note",
     lambda: lay((0, sine_track(N("A5") * 2 ** ((-5 * np.sin(np.pi * np.clip(tvec(0.25) / 0.22, 0, 1)) - 0) / 12)) * ml.env(0.25, 0.02, 0.08, 0.1), -4),
                 (0.2, v_vibe(N("A5"), 0.6, 0.22, depth=0.12), 0)), (-17, 0.6)),
    ("page-turn airy sweep with a marimba chord landing (D5 F#5 A5)",
     lambda: lay((0, v_air(0.22, 1500, 6500, 2.2, curve=0.8) * (1 + 0.5 * np.sin(2 * np.pi * 28 * tvec(0.22))), -5),
                 (0.2, chord(lambda f: v_marimba(f, 0.55, 0.2), [N("D5"), N("F#5"), N("A5")], 0.012), 0)), (-17, 0.6)),
    ("portal shimmer: rising sparkle cloud landing on a low bell (F4)",
     lambda: lay((0, v_sparkle(0.3, 12, 2500, 7000, seed=1), -8), (0.26, v_bell(N("F5"), 0.6, 0.2), 0),
                 (0.26, v_ding(N("F4"), 0.6, 0.2), -6)), (-15, 0.75)),
    ("portal shimmer: detuned chorus rising an octave into a bell (C5 -> C6)",
     lambda: lay((0, sum(sine_track(pitch_curve(N("C6") * (1 + d), 0.32, -12, 0.12)) for d in (-0.004, 0, 0.005))
                  * ml.env(0.32, 0.12, 0.06, 0.12), -6), (0.28, v_bell(N("C6"), 0.6, 0.2), 0)), (-15, 0.7)),
    ("kalimba run up C5 D5 E5 G5 C6",
     lambda: lay(*arp(lambda f: v_kalimba(f, 0.55, 0.2), [N("C5"), N("D5"), N("E5"), N("G5"), N("C6")], 0.04,
                      [-5, -5, -4, -3, 0])), (-17, 0.6)),
    ("kalimba moving figure E5 G5 A5 D6",
     lambda: lay(*arp(lambda f: v_kalimba(f, 0.55, 0.2), [N("E5"), N("G5"), N("A5"), N("D6")], 0.07,
                      [-3, -3, -3, 0])), (-17, 0.6)),
    ("vibraphone arpeggio F4 A4 C5 E5 G5 with tremolo",
     lambda: lay(*arp(lambda f: v_vibe(f, 0.7, 0.24), [N("F4"), N("A4"), N("C5"), N("E5"), N("G5")], 0.055,
                      [-5, -5, -4, -3, 0])), (-16, 0.7)),
    ("wide e-piano Eb4 Bb4 G5 with a sparkle tail",
     lambda: lay(*arp(lambda f: v_epiano(f, 0.75, 0.25, index=0.9), [N("Eb4"), N("Bb4"), N("G5")], 0.09, [-4, -3, 0]),
                 (0.2, v_sparkle(0.25, 6, 4000, 7500, seed=2), -18)), (-15, 0.7)),
    ("marimba roll ascending G4 B4 D5 G5 B5 D6, quick",
     lambda: lay(*arp(lambda f: v_marimba(f, 0.55, 0.15), [N("G4"), N("B4"), N("D5"), N("G5"), N("B5"), N("D6")], 0.032,
                      [-6, -6, -5, -4, -3, 0])), (-17, 0.6)),
    ("soft whoosh with a gliding chime 'arrival' (B5)",
     lambda: lay((0, v_air(0.35, 300, 3000, 1.2, curve=0.5), -5),
                 (0.22, sine_track(pitch_curve(N("B5"), 0.6, 2, 0.03)) * env2(0.6, 0.004, 0.16, tail_db=-14, tail_tau=0.35), 0),
                 (0.22, v_glass(N("F#6"), 0.4, 0.1), -10)), (-16, 0.7)),
    ("glass and e-piano layered arpeggio G4 D5 G5 B5",
     lambda: lay(*arp(lambda f: smx(v_epiano(f, 0.7, 0.22, tine_db=-40), 0.3 * v_glass(f * 2, 0.4, 0.1)),
                      [N("G4"), N("D5"), N("G5"), N("B5")], 0.065, [-4, -4, -3, 0])), (-15, 0.7)),
    ("felt piano Cadd9 broad arpeggio C4 G4 D5 E5, roomy",
     lambda: lay(*arp(lambda f: v_felt(f, 0.75, 0.24), [N("C4"), N("G4"), N("D5"), N("E5")], 0.095, [-3, -3, -2, 0])), (-14, 0.8)),
]

FAMILIES = {"question": QUESTION, "reply_finished": REPLY, "session_changed": SESSION}


# ------------------------------------------------------------------ finishing (48 kHz)

def limit(x, ceil, look=0.003):
    """Click-free peak limiter: running minimum of the needed gain over +-look, then a
    Hann smoothing of the same width. Every smoothed value averages minima of windows
    that contain the sample, so it never exceeds the gain that sample needs, and the
    gain curve has no steps (no zipper clicks)."""
    from scipy.ndimage import minimum_filter1d
    need = np.minimum(1.0, ceil / (np.abs(x) + 1e-12))
    w = int(look * SR)
    g = minimum_filter1d(need, size=2 * w + 1, mode="nearest")
    k = np.hanning(2 * w + 3)[1:-1]
    k /= k.sum()
    g = np.convolve(np.pad(g, w, mode="edge"), k, mode="valid")
    return x * g


def decimate(x):
    y = ml.lowpass(np.asarray(x, float), 16000, order=8)
    return signal.resample_poly(y, 1, OS)


def finish(x, target, max_s, lead=0.004):
    sos = signal.butter(2, 30, "highpass", fs=SR, output="sos")
    x = signal.sosfilt(sos, x)
    a = np.abs(x)
    last = np.nonzero(a > a.max() * 10 ** (-62 / 20))[0][-1]
    x = x[: last + int(0.004 * SR)]
    room = int((max_s - 2 * lead) * SR)
    if len(x) > room:  # let the tail close smoothly inside the length budget
        nf = min(int(0.30 * room), int(0.12 * SR))
        x = x[:room]
        x[-nf:] *= (0.5 + 0.5 * np.cos(np.linspace(0, np.pi, nf))) ** 1.5
    else:
        nf = min(int(0.01 * SR), len(x) // 4)
        x[-nf:] *= 0.5 + 0.5 * np.cos(np.linspace(0, np.pi, nf))
    ni = int(0.0005 * SR)
    x[:ni] *= 0.5 - 0.5 * np.cos(np.linspace(0, np.pi, ni))
    ceil = 10 ** (CEIL_DB / 20)
    x0 = x.copy()
    for _ in range(6):
        x = x * 10 ** ((target - mp.loud_s100(x)) / 20)
        if np.abs(x).max() > ceil:
            x = limit(x, ceil * 0.995)
    if np.abs(x).max() > ceil:
        x *= ceil / np.abs(x).max()
    gr = 20 * np.log10(np.abs(x0).max() * (np.abs(x).sum() / np.abs(x0).sum()) / np.abs(x).max())
    x = np.concatenate([np.zeros(int(lead * SR)), x, np.zeros(int(lead * SR))])
    h = np.hanning(len(x))
    x = x - x.mean() * h / h.mean()
    x[0] = x[-1] = 0.0
    return x, float(gr)


def render_all(outdir):
    os.makedirs(outdir, exist_ok=True)
    labels, report = {}, {}
    for fam, items in FAMILIES.items():
        for i, (desc, fn, space) in enumerate(items, 1):
            y = decimate(tidy(fn()))
            if space is not None:
                wet, rt = space
                y = mp.add_space(y, wet, rt60=rt, damp_hz=4000.0)
            y, gr = finish(y, TARGET[fam], DUR[fam][1])
            name = f"{fam}-{i:02d}.wav"
            ml.write_wav(os.path.join(outdir, name), y)
            labels[name] = desc
            report[name] = gr
    with open(os.path.join(outdir, "labels.json"), "w", encoding="utf-8") as f:
        json.dump(labels, f, indent=2)
    return report


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "out")
    rep = render_all(out)
    for k, v in rep.items():
        if v > 2.0:
            print(f"{k}: limiter gain reduction {v:.1f} dB")
    print("rendered", len(rep))
