"""Routing between Chatterbox and the Kokoro fallback in WinTtsBackend.run(), plus
the daemon's once-per-run spoken fallback notice. Mocks at the seams (sonara.chatterbox
functions, _get_kokoro, _play_wav_bytes) so no real chatterbox venv/worker, winrt, or
Kokoro model is touched. Mirrors the fixture pattern in test_win_tts_kokoro.py."""
import pytest

from sonara.platform.windows import tts as wtts
from sonara import chatterbox
from sonara.router import CONTROL

from tests.daemon_helpers import make_daemon


def _bare_backend():
    # Skip __init__ (which sweeps temp WAVs / touches the FS); we only test routing.
    b = wtts.WinTtsBackend.__new__(wtts.WinTtsBackend)
    b._synth = None
    b._kokoro = None
    return b


# --- tts.run routing ---------------------------------------------------------

def test_chatterbox_voice_routes_to_worker(monkeypatch):
    b = _bare_backend()
    seen = {}
    calls = []

    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: True)
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(chatterbox, "gate_ok", lambda cfg: True)

    def _synth_wav(text, name, cfg):
        calls.append((text, name))
        return b"RIFF..."

    monkeypatch.setattr(chatterbox.CLIENT, "synth_wav", _synth_wav)
    monkeypatch.setattr(b, "_get_kokoro",
                        lambda: pytest.fail("kokoro used for a chatterbox voice"))
    monkeypatch.setattr(wtts, "_play_wav_bytes", lambda data: seen.setdefault("played", data))

    played_order = []

    def _on_play():
        played_order.append("on_play")

    b.run("hello there", "calm-lady", 200, on_play=_on_play)

    assert calls == [("hello there", "calm-lady")]
    assert seen["played"] == b"RIFF..."
    assert played_order == ["on_play"]


def test_gate_failure_falls_back_to_kokoro(monkeypatch):
    from sonara import kokoro
    b = _bare_backend()
    seen = {}
    synth_calls = []

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: True)
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(chatterbox, "gate_ok", lambda cfg: False)
    monkeypatch.setattr(chatterbox.CLIENT, "synth_wav",
                        lambda *a: pytest.fail("worker must not be called when gated off"))

    class FakeEngine:
        def wav_bytes(self, text, voice, speed):
            synth_calls.append((text, voice, speed))
            return b"KOKORO_WAV"

    monkeypatch.setattr(b, "_get_kokoro", lambda: FakeEngine())
    monkeypatch.setattr(wtts, "_play_wav_bytes", lambda data: seen.setdefault("played", data))

    notice = []
    monkeypatch.setattr(chatterbox, "_set_fallback_notice", lambda reason: notice.append(reason))

    b.run("hi", "calm-lady", 200)

    assert synth_calls[0][1] == "af_heart"
    assert seen["played"] == b"KOKORO_WAV"
    assert notice and notice[0]


def test_worker_error_falls_back_to_kokoro(monkeypatch):
    from sonara import kokoro
    b = _bare_backend()
    seen = {}

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: True)
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(chatterbox, "gate_ok", lambda cfg: True)

    def _synth_wav(text, name, cfg):
        raise chatterbox.ChatterboxError("worker crashed")

    monkeypatch.setattr(chatterbox.CLIENT, "synth_wav", _synth_wav)

    class FakeEngine:
        def wav_bytes(self, text, voice, speed):
            return b"KOKORO_WAV"

    monkeypatch.setattr(b, "_get_kokoro", lambda: FakeEngine())
    monkeypatch.setattr(wtts, "_play_wav_bytes", lambda data: seen.setdefault("played", data))

    notice = []
    monkeypatch.setattr(chatterbox, "_set_fallback_notice", lambda reason: notice.append(reason))

    b.run("hi", "calm-lady", 200)

    assert seen["played"] == b"KOKORO_WAV"
    assert notice == ["worker crashed"]


def test_not_provisioned_falls_back(monkeypatch):
    from sonara import kokoro
    b = _bare_backend()
    seen = {}

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: True)
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: False)
    monkeypatch.setattr(chatterbox, "gate_ok",
                        lambda cfg: pytest.fail("gate must not be checked when not provisioned"))
    monkeypatch.setattr(chatterbox.CLIENT, "synth_wav",
                        lambda *a: pytest.fail("worker must not be spawned when not provisioned"))

    class FakeEngine:
        def wav_bytes(self, text, voice, speed):
            return b"KOKORO_WAV"

    monkeypatch.setattr(b, "_get_kokoro", lambda: FakeEngine())
    monkeypatch.setattr(wtts, "_play_wav_bytes", lambda data: seen.setdefault("played", data))

    notice = []
    monkeypatch.setattr(chatterbox, "_set_fallback_notice", lambda reason: notice.append(reason))

    b.run("hi", "calm-lady", 200)

    assert seen["played"] == b"KOKORO_WAV"
    assert notice and notice[0]


def test_list_voices_includes_chatterbox_when_provisioned(monkeypatch):
    from sonara import kokoro
    b = _bare_backend()
    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(b, "_all_voice_infos", lambda: [])
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(chatterbox, "list_voices", lambda: ["cb_default", "x"])

    voices = b.list_voices()

    assert "cb_default" in voices
    assert "x" in voices


def test_list_voices_excludes_chatterbox_when_not_provisioned(monkeypatch):
    from sonara import kokoro
    b = _bare_backend()
    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(b, "_all_voice_infos", lambda: [])
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: False)
    monkeypatch.setattr(chatterbox, "list_voices",
                        lambda: pytest.fail("registry must not be scanned when not provisioned"))

    voices = b.list_voices()

    assert "cb_default" not in voices


# --- daemon once-per-run notice ------------------------------------------------

def test_daemon_speaks_fallback_notice_once(monkeypatch):
    daemon, queue, speaker, sessions, config = make_daemon()
    chatterbox._set_fallback_notice("not provisioned")

    daemon._speak_loop_once()
    daemon._speak_loop_once()

    ch = daemon.router.channel(CONTROL)
    cues = [item.text for item in ch.items if "Chatterbox unavailable" in item.text]
    assert len(cues) == 1
    assert chatterbox.pop_fallback_notice() is None
