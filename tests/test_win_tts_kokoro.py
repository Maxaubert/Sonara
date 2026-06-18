"""WinTtsBackend routing between the native WinRT engine and Kokoro, by voice name.
Mocks at the seams (_get_kokoro / _synthesize_wav / _play_wav_bytes) so no real
winrt, winsound, or Kokoro model is touched."""
import pytest

from sonari.platform.windows import tts as wtts
from sonari import kokoro


def _bare_backend():
    # Skip __init__ (which sweeps temp WAVs / touches the FS); we only test routing.
    b = wtts.WinTtsBackend.__new__(wtts.WinTtsBackend)
    b._synth = None
    b._kokoro = None
    return b


def test_run_routes_a_kokoro_voice_to_the_kokoro_engine(monkeypatch):
    b = _bare_backend()
    seen = {}

    class FakeEngine:
        def wav_bytes(self, text, voice, speed):
            seen["synth"] = (text, voice, speed)
            return b"KOKORO_WAV"

    monkeypatch.setattr(b, "_get_kokoro", lambda: FakeEngine())
    monkeypatch.setattr(b, "_synthesize_wav",
                        lambda *a: pytest.fail("native engine used for a Kokoro voice"))
    monkeypatch.setattr(wtts, "_play_wav_bytes", lambda data: seen.setdefault("played", data))

    b.run("hello there", "af_heart", 200)
    assert seen["synth"][0] == "hello there"
    assert seen["synth"][1] == "af_heart"
    assert seen["synth"][2] == pytest.approx(1.0)        # rate 200 -> speed 1.0
    assert seen["played"] == b"KOKORO_WAV"


def test_run_routes_a_native_voice_to_winrt(monkeypatch):
    b = _bare_backend()
    seen = {}
    monkeypatch.setattr(wtts, "_require_winrt", lambda: None)
    monkeypatch.setattr(b, "_synthesize_wav",
                        lambda text, voice, rate: b"NATIVE_WAV")
    monkeypatch.setattr(b, "_get_kokoro",
                        lambda: pytest.fail("Kokoro used for a native voice"))
    monkeypatch.setattr(wtts, "_play_wav_bytes", lambda data: seen.setdefault("played", data))

    b.run("hi", "Microsoft David", 200)
    assert seen["played"] == b"NATIVE_WAV"


def test_list_voices_includes_native_and_kokoro(monkeypatch):
    b = _bare_backend()
    monkeypatch.setattr(b, "_all_voice_infos", lambda: [])   # no native voices
    voices = b.list_voices()
    assert "af_heart" in voices and "af_nicole" in voices
    assert set(kokoro.VOICES) <= set(voices)


def test_list_voices_survives_no_winrt(monkeypatch):
    b = _bare_backend()
    monkeypatch.setattr(b, "_all_voice_infos",
                        lambda: (_ for _ in ()).throw(ImportError("no winrt")))
    voices = b.list_voices()           # must not raise
    assert "af_heart" in voices
