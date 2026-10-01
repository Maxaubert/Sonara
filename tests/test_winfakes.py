import pytest

import tests._winfakes as _winfakes


@pytest.fixture(autouse=True)
def _fake_windows(monkeypatch):
    # Hermetic on real Windows too (E22): fake winrt + winsound, never live
    # OneCore or the speakers. Live checks live in test_win_tts_live.py.
    _winfakes.force(monkeypatch)


def test_winfakes_make_winrt_and_winsound_importable():
    import tests._winfakes as wf
    wf.install()  # idempotent
    import winsound
    assert hasattr(winsound, "PlaySound")
    from winrt.windows.media.speechsynthesis import SpeechSynthesizer
    SpeechSynthesizer()
    assert list(SpeechSynthesizer.all_voices)


def test_winfakes_are_forced_on_real_windows_too():
    # E22: on win32 install() is a no-op, so without forcing, the TTS tests
    # drove live OneCore and winsound and failed on boxes whose voices cannot
    # synthesize. The autouse fixture must swap the fakes in on every platform.
    import winsound
    from winrt.windows.media.speechsynthesis import SpeechSynthesizer
    assert hasattr(winsound, "_calls"), "real winsound leaked into the test"
    assert SpeechSynthesizer.__module__ == "tests._winfakes"
