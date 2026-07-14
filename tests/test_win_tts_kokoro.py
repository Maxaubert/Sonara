"""WinTtsBackend routing between the native WinRT engine and Kokoro, by voice name.
Mocks at the seams (_get_kokoro / _synthesize_wav / _play_wav_bytes) so no real
winrt, winsound, or Kokoro model is touched."""
import pytest

from sonara.platform.windows import tts as wtts
from sonara import kokoro
from sonara import kokoro_provision as kp


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

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)   # extra present
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
    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(b, "_all_voice_infos", lambda: [])   # no native voices
    voices = b.list_voices()
    assert "af_heart" in voices and "af_nicole" in voices
    assert set(kokoro.VOICES) <= set(voices)


def test_list_voices_excludes_kokoro_without_extra(monkeypatch):
    # Base install (no [kokoro] extra, no venv): don't advertise voices that can't
    # synthesize — otherwise the user picks one and first speak silently fails.
    b = _bare_backend()
    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    monkeypatch.setattr(kp, "neural_enabled", lambda: False)  # isolate the is_installed gate

    class _V:
        display_name = "Microsoft David"
    monkeypatch.setattr(b, "_all_voice_infos", lambda: [_V()])
    voices = b.list_voices()
    assert voices == ["Microsoft David"]
    assert not any(v in voices for v in kokoro.VOICES)


def test_list_voices_lists_kokoro_when_venv_provisioned_without_extra(monkeypatch):
    # CLI on system python (is_installed False) but the daemon's venv is provisioned:
    # advertise the neural voices so they're discoverable (gate on neural_enabled).
    b = _bare_backend()
    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    monkeypatch.setattr(kp, "neural_enabled", lambda: True)
    monkeypatch.setattr(b, "_all_voice_infos", lambda: [])   # no native voices
    voices = b.list_voices()
    assert "af_heart" in voices
    assert set(kokoro.VOICES) <= set(voices)


def test_list_voices_survives_no_winrt(monkeypatch):
    b = _bare_backend()
    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(b, "_all_voice_infos",
                        lambda: (_ for _ in ()).throw(ImportError("no winrt")))
    voices = b.list_voices()           # must not raise
    assert "af_heart" in voices


def test_run_kokoro_voice_without_extra_falls_back_to_winrt(monkeypatch):
    # A Kokoro voice reaching run() without the extra (e.g. via config-sync or a
    # hand-edited config) must NOT raise into the speak loop (which for an
    # eyes-free user meant silence/error noise): it falls back to the native
    # WinRT voice and arms the once-per-run spoken notice (#29). The actionable
    # RuntimeError from require_installed still lands in the notice/log.
    b = _bare_backend()
    monkeypatch.setattr(kokoro, "is_installed", lambda: False)
    monkeypatch.setattr(b, "_get_kokoro",
                        lambda: pytest.fail("must not build the engine without the extra"))
    monkeypatch.setattr(wtts, "_require_winrt", lambda: None)
    monkeypatch.setattr(b, "_synthesize_wav",
                        lambda text, voice, rate: b"WINRT-WAV")
    played = []
    monkeypatch.setattr(wtts, "_play_wav_bytes",
                        lambda data: played.append(data) or "handle")
    assert b.run("hi", "af_heart", 200) == "handle"
    assert played == [b"WINRT-WAV"]
    notice = kokoro.pop_fallback_notice()
    assert notice and "kokoro" in notice.lower()          # actionable reason kept
