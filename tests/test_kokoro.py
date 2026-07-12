import io
import wave

import numpy as np
import pytest

from sonara import kokoro


def test_voices_has_28_including_af_heart_and_asmr():
    assert "af_heart" in kokoro.VOICES          # default, top-rated
    assert "af_nicole" in kokoro.VOICES         # the ASMR/whisper voice
    assert len(kokoro.VOICES) == 28
    assert len(set(kokoro.VOICES)) == 28        # no dupes


def test_is_kokoro_voice():
    assert kokoro.is_kokoro_voice("af_heart")
    assert kokoro.is_kokoro_voice("kokoro:af_heart")     # engine-prefixed form
    assert kokoro.is_kokoro_voice("AF_HEART")            # case-insensitive
    assert not kokoro.is_kokoro_voice("Microsoft David")
    assert not kokoro.is_kokoro_voice("")
    assert not kokoro.is_kokoro_voice(None)


def test_normalize_voice_strips_engine_prefix():
    assert kokoro.normalize_voice("kokoro:af_heart") == "af_heart"
    assert kokoro.normalize_voice("af_heart") == "af_heart"
    assert kokoro.normalize_voice("KOKORO:AF_NICOLE") == "af_nicole"


def test_rate_to_speed_maps_and_clamps():
    assert kokoro.rate_to_speed(200) == pytest.approx(1.0)
    assert kokoro.rate_to_speed(300) == pytest.approx(1.5)
    assert kokoro.rate_to_speed(100) == pytest.approx(0.5)
    assert kokoro.rate_to_speed(2000) <= 2.0      # clamped high
    assert kokoro.rate_to_speed(1) >= 0.5         # clamped low


def test_to_wav_bytes_is_valid_16bit_mono_wav():
    audio = np.array([0.0, 0.5, -0.5, 1.0, -1.0], dtype=np.float32)
    data = kokoro.to_wav_bytes(audio, 24000)
    with wave.open(io.BytesIO(data)) as w:
        assert w.getnchannels() == 1
        assert w.getsampwidth() == 2          # 16-bit
        assert w.getframerate() == 24000
        assert w.getnframes() == 5


class _FakeKokoro:
    def __init__(self):
        self.calls = []

    def create(self, text, voice, speed, lang):
        self.calls.append((text, voice, speed, lang))
        return (np.array([0.1, 0.2, 0.3], dtype=np.float32), 24000)


def test_engine_is_lazy_and_synth_calls_create(tmp_path):
    made = []
    fake = _FakeKokoro()

    def factory(model_path, voices_path):
        made.append((model_path, voices_path))
        return fake

    eng = kokoro.KokoroEngine(tmp_path, factory=factory, ensure=lambda: None)
    assert made == []                          # nothing loaded until first synth
    audio, sr = eng.synth("hello", "kokoro:af_heart", 1.0)
    assert len(made) == 1                       # loaded once
    assert fake.calls == [("hello", "af_heart", 1.0, "en-us")]  # prefix normalized
    assert sr == 24000 and list(audio) == pytest.approx([0.1, 0.2, 0.3])


def test_engine_wav_bytes_roundtrips(tmp_path):
    eng = kokoro.KokoroEngine(
        tmp_path, factory=lambda m, v: _FakeKokoro(), ensure=lambda: None)
    data = eng.wav_bytes("hi", "af_heart", 1.0)
    with wave.open(io.BytesIO(data)) as w:
        assert w.getframerate() == 24000 and w.getnframes() == 3


# --- optional [kokoro] extra: availability gate -----------------------------

def test_is_installed_reflects_find_spec(monkeypatch):
    monkeypatch.setattr(kokoro.importlib.util, "find_spec", lambda name: object())
    assert kokoro.is_installed() is True
    monkeypatch.setattr(kokoro.importlib.util, "find_spec", lambda name: None)
    assert kokoro.is_installed() is False


def test_require_installed_raises_actionable_when_absent(monkeypatch):
    # When the extra is missing, raise a RuntimeError that names the fix — not the
    # raw ModuleNotFoundError the daemon would swallow into silent no-speech.
    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    with pytest.raises(RuntimeError) as ei:
        kokoro.require_installed()
    assert "kokoro" in str(ei.value).lower()      # mentions the extra to install


def test_require_installed_noop_when_present(monkeypatch):
    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    kokoro.require_installed()                     # must not raise


def test_synth_bisects_on_token_overflow():
    # kokoro_onnx's batch splitter can emit a >510-phoneme batch on unusual
    # text (multi-paragraph digests hit this), and _create_audio then raises
    # IndexError: index 510 out of bounds. synth() must recover by bisecting
    # the text and concatenating the halves instead of failing the utterance.
    import numpy as np
    from sonara.kokoro import KokoroEngine

    class FakeK:
        def __init__(self):
            self.calls = []

        def create(self, text, voice, speed, lang):
            self.calls.append(text)
            if len(text) > 40:                      # "too long" stand-in
                raise IndexError("index 510 is out of bounds for axis 0 with size 510")
            return np.ones(8, dtype=np.float32), 24000

    eng = KokoroEngine.__new__(KokoroEngine)
    eng._k = FakeK()
    import threading
    eng._lock = threading.Lock()
    long_text = ("Sentence one is here. " * 4).strip()   # > 40 chars
    audio, sr = eng.synth(long_text, "af_heart")
    assert sr == 24000
    assert len(audio) >= 16                        # both halves concatenated
    assert len(eng._k.calls) >= 3                  # whole, then bisected halves


def test_synth_overflow_single_word_reraises():
    # A single unsplittable chunk that still overflows must raise (nothing to
    # bisect) - the speak loop's failure path handles it.
    import pytest
    from sonara.kokoro import KokoroEngine

    class AlwaysOverflow:
        def create(self, text, voice, speed, lang):
            raise IndexError("index 510 is out of bounds")

    eng = KokoroEngine.__new__(KokoroEngine)
    eng._k = AlwaysOverflow()
    import threading
    eng._lock = threading.Lock()
    with pytest.raises(IndexError):
        eng.synth("supercalifragilistic", "af_heart")
