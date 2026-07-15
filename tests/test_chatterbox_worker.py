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


def test_isolate_from_own_dir_removes_worker_dir():
    # Regression: the worker's own dir (which holds sonara's chatterbox.py) must
    # be stripped so `import chatterbox` finds the pip package, not sonara's
    # same-named module (live bug: ModuleNotFoundError: No module named 'sonara').
    import os
    here = os.path.dirname(os.path.abspath(w.__file__))
    path = [here, "/some/other/dir", os.path.join(here, "sub", "..")]
    w._isolate_from_own_dir(path)
    assert all(os.path.abspath(p) != here for p in path)
    assert "/some/other/dir" in path        # unrelated entries preserved


def test_use_clean_stdout_reserves_protocol_and_redirects_noise(monkeypatch):
    import sys, io
    real_out, real_err = io.StringIO(), io.StringIO()
    monkeypatch.setattr(sys, "stdout", real_out)
    monkeypatch.setattr(sys, "stderr", real_err)
    proto = w._use_clean_stdout()
    assert proto is real_out                 # protocol keeps the original stdout
    print("library noise")                   # a library-style stdout print
    assert sys.stdout is real_err            # now goes to stderr
    assert "library noise" in real_err.getvalue()
    assert real_out.getvalue() == ""         # protocol channel stays clean


def test_split_text_short_is_one_chunk():
    assert w._split_text("Hello there. How are you?") == ["Hello there. How are you?"]


def test_split_text_packs_sentences_under_limit():
    # many sentences -> multiple chunks, each within the char budget, split on
    # sentence boundaries (chatterbox hallucinates into gibberish on long input).
    text = " ".join("This is sentence number {0} here.".format(i) for i in range(40))
    chunks = w._split_text(text, max_chars=120)
    assert len(chunks) > 1
    assert all(len(c) <= 120 for c in chunks)
    # every sentence preserved (nothing dropped)
    assert "".join(chunks).replace(" ", "") == text.replace(" ", "")


def test_split_text_hard_splits_a_too_long_sentence():
    long_sentence = "word " * 100                      # 500 chars, no terminator
    chunks = w._split_text(long_sentence.strip(), max_chars=90)
    assert len(chunks) > 1 and all(len(c) <= 90 for c in chunks)


def test_split_text_keeps_intra_token_dots():
    # The old regex split at EVERY '.', inserting a space: "3. 14",
    # "daemon. py:123" (#56).
    text = "The value of pi is 3.14 exactly. See daemon.py:123 for it."
    assert w._split_text(text) == [text]


def test_split_text_terminates_unpunctuated_tail():
    # Chatterbox trails into hallucinated audio on unterminated fragments (#56).
    assert w._split_text("Done now") == ["Done now."]
    assert w._split_text("Next steps:") == ["Next steps."]


def test_split_text_drops_punctuation_only_text():
    # model.generate('...') produces unpredictable noise, not silence (#56).
    assert w._split_text("...") == []
    assert w._split_text("?!") == []


def test_synth_of_unspeakable_text_returns_silence_not_generate():
    s = _state()
    out = w.handle_request(s, {"type": "synth", "text": "...",
                               "voice_path": None, "variant": "turbo",
                               "exaggeration": None}, now=lambda: 1.0)
    assert out["ok"] is True
    assert s.model.calls == []                         # never handed to the model
    import base64, io, wave
    with wave.open(io.BytesIO(base64.b64decode(out["wav_b64"]))) as f:
        assert f.getnframes() == 0                     # silence, zero samples


def test_synth_chunks_long_text_and_concatenates_audio():
    # A long digest must be synthesized in pieces and stitched, so the audio is
    # the sum of the chunks (and the model is never handed the whole paragraph).
    s = _state()
    long_text = " ".join("Sentence {0} of the digest.".format(i) for i in range(30))
    out = w.handle_request(s, {"type": "synth", "text": long_text,
                               "voice_path": None, "variant": "turbo",
                               "exaggeration": None}, now=lambda: 1.0)
    assert out["ok"] is True
    assert len(s.model.calls) >= 2                     # chunked, not one shot
    import base64, io, wave
    with wave.open(io.BytesIO(base64.b64decode(out["wav_b64"]))) as f:
        frames = f.getnframes()
    assert frames == 24 * len(s.model.calls)           # concatenated (fake=24/chunk)


def test_warm_request_loads_model_without_synth():
    # Pre-warm: a "warm" request loads the model (paying the cold load) so the
    # first real synth is fast, but produces no audio.
    s = _state()
    out = w.handle_request(s, {"type": "warm", "variant": "turbo"}, now=lambda: 1.0)
    assert out == {"ok": True, "loaded": True}
    assert s.model is not None and s.variant == "turbo"
    assert not getattr(s.model, "calls", [])          # generate() never called


def test_normalize_rms_scales_quiet_and_loud_to_target():
    # (#81) chatterbox clones the clip loudness; voices varied 6.7x live
    import numpy as np
    t = np.linspace(0, 1.0, 24000, dtype="float32")
    quiet = (0.02 * np.sin(2 * np.pi * 220 * t)).astype("float32")
    loud = (0.9 * np.sin(2 * np.pi * 220 * t)).astype("float32")
    for x in (quiet, loud):
        y = w._normalize_rms(x)
        rms = float(np.sqrt((y ** 2).mean()))
        assert 0.045 < rms < 0.09, rms        # sine RMS = amp/sqrt(2) ~ target
        assert float(np.abs(y).max()) <= 0.97


def test_normalize_rms_leaves_silence_alone():
    import numpy as np
    silent = np.zeros(24000, dtype="float32")
    assert np.array_equal(w._normalize_rms(silent), silent)


def test_normalize_rms_matches_kokoro_twin():
    # duplicated on purpose (the worker cannot import sonara); keep in sync
    import numpy as np
    from sonara.kokoro import normalize_rms
    rng = np.linspace(-0.3, 0.3, 48000, dtype="float32")
    x = (rng * np.sin(np.linspace(0, 700, 48000))).astype("float32")
    assert np.allclose(w._normalize_rms(x), normalize_rms(x))
