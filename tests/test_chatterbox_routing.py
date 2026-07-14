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

class _FakeSub:
    """Minimal Popen-like sub-handle for driving _ChatterboxHandle.wait()."""
    def wait(self, timeout=None):
        return 0

    def terminate(self):
        pass


def test_chatterbox_voice_returns_streaming_handle(monkeypatch):
    b = _bare_backend()

    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: True)
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(chatterbox.CLIENT, "synth_wav",
                        lambda *a: pytest.fail("synth must not start before wait()"))
    monkeypatch.setattr(b, "_get_kokoro",
                        lambda: pytest.fail("kokoro must not be built for the streaming path"))

    handle = b.run("hello there", "calm-lady", 200)

    assert type(handle).__name__ == "_ChatterboxHandle"


def test_on_play_flows_into_streaming_handle(monkeypatch):
    b = _bare_backend()

    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: True)
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)

    def _on_play():
        pass

    handle = b.run("hello there", "calm-lady", 200, on_play=_on_play)

    assert handle._on_play is _on_play


def test_synth_one_falls_back_to_kokoro_per_chunk(monkeypatch, capsys):
    from sonara import kokoro
    b = _bare_backend()
    synth_calls = []
    played = []

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: True)
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(chatterbox, "split_text",
                        lambda text, max_chars=280: ["c1", "c2", "c3"])

    def _synth_wav(chunk, name, cfg):
        if chunk == "c2":
            raise chatterbox.ChatterboxError("worker crashed")
        return ("CB:" + chunk).encode()

    monkeypatch.setattr(chatterbox.CLIENT, "synth_wav", _synth_wav)

    class FakeEngine:
        def wav_bytes(self, text, voice, speed):
            synth_calls.append((text, voice, speed))
            return ("KOKORO:" + text).encode()

    monkeypatch.setattr(b, "_get_kokoro", lambda: FakeEngine())

    def _play(data):
        played.append(data)
        return _FakeSub()

    monkeypatch.setattr(wtts, "_play_wav_bytes", _play)

    notice = []
    monkeypatch.setattr(chatterbox, "_set_fallback_notice", lambda reason: notice.append(reason))

    handle = b.run("hello there", "calm-lady", 200)
    assert type(handle).__name__ == "_ChatterboxHandle"
    rc = handle.wait()

    assert rc == 0
    assert played == [b"CB:c1", b"KOKORO:c2", b"CB:c3"]
    assert synth_calls == [("c2", "af_heart", pytest.approx(1.0))]
    assert notice == ["worker crashed"]
    assert "[chatterbox] fallback:" in capsys.readouterr().err


def test_not_provisioned_uses_kokoro_with_notice(monkeypatch):
    from sonara import kokoro
    b = _bare_backend()
    seen = {}

    monkeypatch.setattr(kokoro, "is_installed", lambda: True)
    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: True)
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: False)
    monkeypatch.setattr(chatterbox.CLIENT, "synth_wav",
                        lambda *a: pytest.fail("worker must not be spawned when not provisioned"))

    class FakeEngine:
        def wav_bytes(self, text, voice, speed):
            return b"KOKORO_WAV"

    monkeypatch.setattr(b, "_get_kokoro", lambda: FakeEngine())
    monkeypatch.setattr(wtts, "_play_wav_bytes", lambda data: seen.setdefault("played", data))

    notice = []
    monkeypatch.setattr(chatterbox, "_set_fallback_notice", lambda reason: notice.append(reason))

    handle = b.run("hi", "calm-lady", 200)

    assert type(handle).__name__ != "_ChatterboxHandle"
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


# --- kokoro failure fallback (#29) ----------------------------------------------

def test_kokoro_failure_falls_back_to_winrt_and_arms_notice(monkeypatch):
    # (#29) a Kokoro synth failure (e.g. onnxruntime's DLL-order poisoning by
    # winrt's bundled MSVCP140) used to escape run(): unexplained error noise on
    # EVERY utterance, no fallback, no notice. It must fall back to the native
    # WinRT voice and arm the once-per-run spoken notice.
    from sonara import kokoro
    b = _bare_backend()
    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: False)
    monkeypatch.setattr(kokoro, "is_kokoro_voice", lambda name: True)
    monkeypatch.setattr(kokoro, "require_installed", lambda: None)

    class BrokenKokoro:
        def wav_bytes(self, *a, **k):
            raise ImportError("DLL initialization routine failed")
    monkeypatch.setattr(b, "_get_kokoro", lambda: BrokenKokoro())
    monkeypatch.setattr(wtts, "_require_winrt", lambda: None)
    monkeypatch.setattr(b, "_synthesize_wav",
                        lambda text, voice, rate: b"WINRT-WAV")
    played = []
    monkeypatch.setattr(wtts, "_play_wav_bytes",
                        lambda data: played.append(data) or "handle")

    out = b.run("hello", "af_heart", 200)
    assert out == "handle"
    assert played == [b"WINRT-WAV"]                    # WinRT spoke instead
    assert kokoro.pop_fallback_notice()                # notice armed...
    assert kokoro.pop_fallback_notice() is None        # ...once


def test_kokoro_success_does_not_arm_notice(monkeypatch):
    from sonara import kokoro
    b = _bare_backend()
    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: False)
    monkeypatch.setattr(kokoro, "is_kokoro_voice", lambda name: True)
    monkeypatch.setattr(kokoro, "require_installed", lambda: None)

    class OkKokoro:
        def wav_bytes(self, *a, **k):
            return b"KOKORO-WAV"
    monkeypatch.setattr(b, "_get_kokoro", lambda: OkKokoro())
    monkeypatch.setattr(wtts, "_play_wav_bytes", lambda data: "handle")

    assert b.run("hello", "af_heart", 200) == "handle"
    assert kokoro.pop_fallback_notice() is None


def test_daemon_speaks_kokoro_fallback_notice_once(monkeypatch):
    from sonara import kokoro
    daemon, queue, speaker, sessions, config = make_daemon()
    kokoro._set_fallback_notice("DLL initialization routine failed")

    daemon._speak_loop_once()
    daemon._speak_loop_once()

    ch = daemon.router.channel(CONTROL)
    cues = [item.text for item in ch.items if "Kokoro unavailable" in item.text]
    assert len(cues) == 1
    assert kokoro.pop_fallback_notice() is None


def test_chatterbox_voice_always_tries_chatterbox(monkeypatch):
    # (#49) VRAM gate removed: a provisioned Chatterbox voice ALWAYS returns the
    # streaming handle -- no VRAM check can route it to Kokoro. Only a real
    # per-chunk synth error (tested elsewhere) falls back.
    b = _bare_backend()
    monkeypatch.setattr(chatterbox, "is_chatterbox_voice", lambda name: True)
    monkeypatch.setattr(chatterbox, "is_provisioned", lambda: True)
    monkeypatch.setattr(chatterbox.CLIENT, "synth_wav",
                        lambda *a: pytest.fail("synth must not start before wait()"))
    monkeypatch.setattr(b, "_get_kokoro",
                        lambda: pytest.fail("must not route to Kokoro when provisioned"))
    assert not hasattr(chatterbox, "gate_ok")            # gate function is gone
    handle = b.run("hello there", "shadowheart", 200)
    assert isinstance(handle, wtts._ChatterboxHandle)
