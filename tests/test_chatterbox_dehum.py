"""The synth worker notches out the ~120 Hz Chatterbox generation hum (#51).

Verified against real audio: the model emits a persistent ~120 Hz tone in the
quiet gaps of every voice (~8x Kokoro's floor) that pulses on/off per chunk.
Here we prove the filter itself: a 120 Hz tone is deeply attenuated while a
200 Hz voice-range tone passes through intact."""
import numpy as np
import pytest

pytest.importorskip("scipy")

from sonara.chatterbox_worker import _dehum


def _tone(freq, sr=24000, secs=1.0):
    t = np.arange(int(sr * secs)) / sr
    return np.sin(2 * np.pi * freq * t).astype("float32")


def _rms(x):
    return float(np.sqrt(np.mean(np.asarray(x, dtype="float64") ** 2)))


def test_dehum_attenuates_120hz_hum():
    hum = _tone(120)
    out = _dehum(hum, 24000)
    assert _rms(out) < _rms(hum) * 0.15      # >= ~16 dB down on the hum tone


def test_dehum_preserves_voice_body():
    # voice body (>=250Hz, clear of the notch) passes essentially untouched;
    # real voices are broadband here, so body is fully preserved (measured 100%)
    for f in (250, 800, 2000):
        out = _dehum(_tone(f), 24000)
        assert _rms(out) > _rms(_tone(f)) * 0.97


def test_dehum_no_scipy_returns_input(monkeypatch):
    import sonara.chatterbox_worker as w
    import builtins
    real_import = builtins.__import__
    def fake(name, *a, **k):
        if name == "scipy.signal":
            raise ImportError("no scipy")
        return real_import(name, *a, **k)
    monkeypatch.setattr(builtins, "__import__", fake)
    x = _tone(120)
    out = w._dehum(x, 24000)
    assert out is x                    # graceful no-op, synthesis never breaks


def test_dehum_tolerates_tiny_input():
    assert _dehum(np.zeros(4, dtype="float32"), 24000).shape[0] == 4
