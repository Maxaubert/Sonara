"""Chatterbox worker logic, driven with a fake model - no torch, no GPU.
The worker runs inside the chatterbox venv in production; these tests import
the module on the repo venv, which must work because torch imports happen
lazily inside the loader."""
import base64
import json

from sonara import chatterbox_worker as w


class FakeModel:
    sr = 24000

    def __init__(self):
        self.calls = []

    def generate(self, text, audio_prompt_path=None, exaggeration=None):
        self.calls.append({"text": text, "prompt": audio_prompt_path,
                           "exaggeration": exaggeration})
        class T:  # minimal tensor stand-in: squeeze().cpu().numpy() -> list
            def squeeze(self): return self
            def cpu(self): return self
            def numpy(self):
                import numpy as np
                return np.zeros(24, dtype="float32")
        return T()


def _state(**kw):
    s = w.WorkerState(idle_unload_s=kw.get("idle_unload_s", 600))
    s.loader = lambda variant: FakeModel()
    return s


def test_ping_reports_not_loaded():
    s = _state()
    out = w.handle_request(s, {"type": "ping"}, now=lambda: 100.0)
    assert out == {"ok": True, "loaded": False}


def test_synth_loads_lazily_and_returns_wav_b64():
    s = _state()
    out = w.handle_request(s, {"type": "synth", "text": "Hi there.",
                               "voice_path": None, "variant": "turbo",
                               "exaggeration": None}, now=lambda: 100.0)
    assert out["ok"] is True
    wav = base64.b64decode(out["wav_b64"])
    assert wav[:4] == b"RIFF"                       # real WAV container
    assert s.model is not None                      # stayed resident


def test_synth_passes_voice_and_exaggeration():
    s = _state()
    w.handle_request(s, {"type": "synth", "text": "T", "voice_path": "C:/v.wav",
                         "variant": "turbo", "exaggeration": 0.7},
                     now=lambda: 100.0)
    call = s.model.calls[0]
    assert call["prompt"] == "C:/v.wav" and call["exaggeration"] == 0.7


def test_variant_switch_reloads():
    s = _state()
    w.handle_request(s, {"type": "synth", "text": "a", "voice_path": None,
                         "variant": "turbo", "exaggeration": None},
                     now=lambda: 100.0)
    first = s.model
    w.handle_request(s, {"type": "synth", "text": "b", "voice_path": None,
                         "variant": "original", "exaggeration": None},
                     now=lambda: 101.0)
    assert s.model is not first and s.variant == "original"


def test_idle_unload_frees_model():
    s = _state(idle_unload_s=60)
    w.handle_request(s, {"type": "synth", "text": "a", "voice_path": None,
                         "variant": "turbo", "exaggeration": None},
                     now=lambda: 100.0)
    w.maybe_unload(s, now=lambda: 159.0)
    assert s.model is not None                      # not idle long enough
    w.maybe_unload(s, now=lambda: 161.0)
    assert s.model is None                          # unloaded


def test_synth_error_returns_ok_false():
    s = _state()
    def boom(variant):
        raise RuntimeError("CUDA out of memory")
    s.loader = boom
    out = w.handle_request(s, {"type": "synth", "text": "a", "voice_path": None,
                               "variant": "turbo", "exaggeration": None},
                           now=lambda: 100.0)
    assert out["ok"] is False and "CUDA out of memory" in out["error"]


def test_unknown_request_type_is_an_error():
    out = w.handle_request(_state(), {"type": "dance"}, now=lambda: 100.0)
    assert out["ok"] is False
