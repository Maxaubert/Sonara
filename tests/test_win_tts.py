"""WinTtsBackend (OneCore via PyWinRT) -- mock-tested on macOS via _winfakes.

WINDOWS-only code. "Green" here means the MOCKED contract holds (the fake
winrt tree injected by tests/_winfakes.py); it is NOT a claim that OneCore TTS
works on real Windows.
"""
from __future__ import annotations

import subprocess

import pytest

from sonara.platform.windows.tts import WinTtsBackend, wpm_to_speaking_rate


def test_list_voices():
    b = WinTtsBackend()
    assert isinstance(b.list_voices(), list) and b.list_voices()


def test_best_voice_returns_display_name_string():
    # ABC contract: best_voice() -> str. Holds for both the macOS fake voice
    # and a real OneCore voice on Windows (no hard-coded name).
    b = WinTtsBackend()
    v = b.best_voice()
    assert isinstance(v, str) and v
    assert v == b._best_voice_info().display_name


def test_best_voice_info_returns_object_with_onecore_id():
    info = WinTtsBackend()._best_voice_info()
    assert "speech_onecore" in (info.id or "").lower()


def test_synthesize_wav_runs_blocking_body_off_calling_thread(monkeypatch):
    # (#60) the daemon speak loop is an STA COM thread, where WinRT's blocking
    # .get() raises "Cannot call blocking method from single-threaded
    # apartment" (live regression: every fast cue played the error earcon).
    # The blocking body must therefore run on a fresh (MTA-capable) thread.
    import threading
    b = WinTtsBackend()
    seen = {}

    def fake_blocking(text, voice, rate):
        seen["thread"] = threading.current_thread()
        return b"WAVBYTES"

    monkeypatch.setattr(b, "_synthesize_wav_blocking", fake_blocking)
    out = b._synthesize_wav("hi", None, 200)
    assert out == b"WAVBYTES"
    assert seen["thread"] is not threading.current_thread()


def test_synthesize_wav_propagates_worker_exceptions(monkeypatch):
    b = WinTtsBackend()

    def boom(text, voice, rate):
        raise RuntimeError("no voices installed")

    monkeypatch.setattr(b, "_synthesize_wav_blocking", boom)
    with pytest.raises(RuntimeError, match="no voices installed"):
        b._synthesize_wav("hi", None, 200)


def test_run_completes_returns_zero():
    h = WinTtsBackend().run("hello", None, 200)
    assert h.wait(timeout=2.0) == 0


def test_terminate_sets_returncode_one():
    h = WinTtsBackend().run("hello", None, 200)
    h.terminate()
    assert h.returncode == 1


def test_wait_timeout_raises(monkeypatch):
    # A "long" clip: the completion timer hasn't fired, so a tiny wait must raise.
    import sonara.platform.windows.tts as tts
    monkeypatch.setattr(tts, "_wav_duration", lambda data: 100.0)
    h = WinTtsBackend().run("hello", None, 200)
    try:
        with pytest.raises(subprocess.TimeoutExpired):
            h.wait(timeout=0.05)
    finally:
        h.terminate()   # cancel the 100s timer so the test doesn't linger


def test_wpm_maps_to_multiplier():
    assert abs(wpm_to_speaking_rate(200) - 1.0) < 1e-6
    assert wpm_to_speaking_rate(400) > 1.0 and wpm_to_speaking_rate(100) < 1.0


def test_run_falls_back_when_voice_name_unknown():
    # a stale/foreign voice name (e.g. macOS "Samantha") must not be assigned
    # as-is to synth.voice -- run() resolves it or falls back to best_voice().
    h = WinTtsBackend().run("hi", "Samantha", 200)  # fake has no such voice
    assert h.wait(timeout=2.0) == 0   # did not crash on an unresolved name


def test_run_raises_actionable_error_when_no_voices(monkeypatch):
    # On a box with no OneCore voices, run() must surface the actionable
    # "install a voice" RuntimeError -- NOT the raw FileNotFoundError that real
    # SpeechSynthesizer activation throws. Regression for the no-voices error.
    import winrt.windows.media.speechsynthesis as ss
    monkeypatch.setattr(ss.SpeechSynthesizer, "all_voices", [])
    with pytest.raises(RuntimeError, match="No TTS voices installed"):
        WinTtsBackend().run("hello", None, 200)


def test_list_voices_returns_display_name_strings():
    # ABC + macOS return list[str]; Windows must match, not VoiceInformation
    # objects (the same object-vs-name slip the PR fixed for best_voice()). (#16)
    voices = WinTtsBackend().list_voices()
    assert isinstance(voices, list) and voices
    assert all(isinstance(v, str) for v in voices), voices


def test_terminate_issues_a_real_stop_playsound_call():
    # SND_PURGE is documented "not supported on modern Windows"; the stop must
    # go through PlaySound(None, 0). The fake winsound has no SND_PURGE, so the
    # old call raised AttributeError and was swallowed -> interrupt never stopped
    # audio, and no test ever caught it. (#17)
    import winsound
    winsound._calls.clear()
    h = WinTtsBackend().run("hello", None, 200)
    played = winsound._calls[-1]
    assert played[1] & winsound.SND_ASYNC, played   # async playback, not SND_SYNC
    h.terminate()
    assert (None, 0) in winsound._calls, winsound._calls  # a real stop was issued


def test_init_sweeps_stale_temp_wavs(tmp_path, monkeypatch):
    # A crashed/killed daemon can leak sonara-tts-*.wav in %TEMP%. Backend init
    # sweeps OLD ones (never a possibly-in-flight recent file, never foreign
    # files). (#26)
    import os, tempfile, time
    monkeypatch.setattr(tempfile, "gettempdir", lambda: str(tmp_path))
    stale = tmp_path / "sonara-tts-OLD.wav"
    fresh = tmp_path / "sonara-tts-NEW.wav"
    foreign = tmp_path / "someone-elses.wav"
    for f in (stale, fresh, foreign):
        f.write_bytes(b"x")
    old = time.time() - 10_000
    os.utime(str(stale), (old, old))
    WinTtsBackend()  # __init__ sweeps
    assert not stale.exists()   # old sonara temp removed
    assert fresh.exists()       # recent one kept (may be in-flight)
    assert foreign.exists()     # non-sonara file untouched


def test_run_raises_actionable_error_when_winrt_missing(monkeypatch):
    # A Windows box without PyWinRT installed must get an actionable error at the
    # synth path, not silent no-speech (doctor also goes red -- see supervisor). (#7)
    import sonara.platform.windows.tts as tts
    monkeypatch.setattr(tts, "_winrt_available", lambda: False)
    with pytest.raises(RuntimeError, match="(?i)pywinrt|winrt"):
        WinTtsBackend().run("hello", None, 200)


def test_pyproject_declares_windows_winrt_extra():
    # winrt is a hard Windows dependency; it must be declared so `pip install`
    # users on Windows actually get speech (not green-doctor + silence). (#7)
    import os
    import tomllib
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    with open(os.path.join(root, "pyproject.toml"), "rb") as fh:
        data = tomllib.load(fh)
    extras = data["project"]["optional-dependencies"]
    assert "windows" in extras, extras
    assert any("winrt" in d.lower() for d in extras["windows"]), extras["windows"]
