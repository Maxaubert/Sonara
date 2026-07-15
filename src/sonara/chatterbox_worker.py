"""Chatterbox GPU worker: newline-JSON over stdin/stdout.

Runs inside ~/.sonara/chatterbox-venv (Python 3.12, torch cu128,
chatterbox-tts). It must NOT import sonara - the venv does not have it.
torch/chatterbox imports happen lazily inside _load_model so the repo test
suite (no torch) can import this module and drive handle_request with fakes.

Protocol (one request per line):
  {"type": "synth", "text", "voice_path", "variant", "exaggeration"}
      -> {"ok": true, "wav_b64": ...} | {"ok": false, "error": ...}
  {"type": "ping"} -> {"ok": true, "loaded": bool}
The model idle-unloads after idle_unload_s without requests (VRAM freed);
the process stays and reloads lazily on the next synth.
"""
from __future__ import annotations

import base64
import io
import json
import sys
import threading
import time
import wave


def _isolate_from_own_dir(path_list):
    """Drop this file's own directory from *path_list* in place. The worker runs
    from the sonara package dir (a plain `python worker.py` puts the script's dir
    on sys.path[0]), and that dir also holds sonara's own `chatterbox.py`. Without
    this, `from chatterbox...` resolves to sonara's module (which imports sonara)
    instead of the pip `chatterbox` package the worker needs -> ModuleNotFoundError:
    No module named 'sonara'. Called right before the real import so tests, which
    inject a fake loader, never touch it."""
    import os
    here = os.path.dirname(os.path.abspath(__file__))
    path_list[:] = [p for p in path_list if os.path.abspath(p) != here]


def _load_model(variant):
    # Verified against docs/superpowers/specs/2026-07-12-chatterbox-smoke.md;
    # adjust there first if the package API changes.
    import sys
    _isolate_from_own_dir(sys.path)
    if variant == "original":
        from chatterbox.tts import ChatterboxTTS
        return ChatterboxTTS.from_pretrained(device="cuda")
    from chatterbox.tts_turbo import ChatterboxTurboTTS
    return ChatterboxTurboTTS.from_pretrained(device="cuda")


_MAX_CHUNK_CHARS = 280


def _split_text(text, max_chars=_MAX_CHUNK_CHARS):
    """Split *text* into speakable chunks no longer than *max_chars*, breaking on
    sentence boundaries. Chatterbox degrades into gibberish on long input (its
    reliable context is short, which is why the demos cap the box), so a whole
    paragraph digest must be synthesized in pieces. A single sentence longer than
    the budget is hard-split on spaces. Kept in sync with sonara.chatterbox
    .split_text (the worker cannot import sonara)."""
    import re
    text = (text or "").strip()
    if not text:
        return []
    # Split at whitespace PRECEDED by a terminator (optionally + closing quote/
    # bracket). The old findall split at EVERY '.', corrupting intra-token dots
    # with an inserted space: "3.14" -> "3. 14", "daemon.py:123" ->
    # "daemon. py:123" (#56). re.split never drops text.
    sentences = re.split(r"(?:(?<=[.!?])|(?<=[.!?][\"')\]]))\s+", text)
    chunks = []
    cur = ""
    for s in sentences:
        s = s.strip()
        if not s:
            continue
        if len(s) > max_chars:
            if cur:
                chunks.append(cur)
                cur = ""
            buf = ""
            for word in s.split(" "):
                if buf and len(buf) + 1 + len(word) > max_chars:
                    chunks.append(buf)
                    buf = word
                else:
                    buf = (buf + " " + word).strip()
            if buf:
                cur = buf
        elif cur and len(cur) + 1 + len(s) > max_chars:
            chunks.append(cur)
            cur = s
        else:
            cur = (cur + " " + s).strip()
    if cur:
        chunks.append(cur)
    out = []
    for c in chunks:
        # Chatterbox renders punctuation-only chunks as unpredictable noise and
        # trails into hallucinated audio on unterminated ones (#56): skip the
        # former, terminate the latter (a turn-final fragment or bullet line).
        if not re.search(r"[A-Za-z0-9]", c):
            continue
        if c[-1] not in ".!?…" and len(c) < max_chars:
            c = c.rstrip(":;,-") + "."
        out.append(c)
    return out


def _to_float_mono(tensor):
    import numpy as np
    return np.asarray(tensor.squeeze().cpu().numpy(), dtype="float32").reshape(-1)


def _normalize_rms(audio, target=0.08, peak=0.97, frame=480, floor=1e-4):
    """Scale float mono audio so its VOICED RMS lands on *target* (~-22 dBFS),
    hard-capped so no sample exceeds *peak*. Chatterbox clones the reference
    clip's loudness, so voices varied wildly (one clip hot, another quiet, #81);
    normalizing the synthesized output makes every voice comparable. Frames
    below a tenth of the loudest frame are pauses/silence and do not count
    (they would understate loudness). Silent/empty audio is returned as-is.
    Kept in sync with sonara.kokoro.normalize_rms (the worker cannot import
    sonara)."""
    import numpy as np
    x = np.asarray(audio, dtype="float32")
    if x.size < frame:
        return x
    frames = x[: (len(x) // frame) * frame].reshape(-1, frame)
    rms = np.sqrt((frames ** 2).mean(axis=1))
    gate = max(floor, float(rms.max()) * 0.1)
    voiced = rms[rms > gate]
    cur = float(voiced.mean()) if voiced.size else 0.0
    if cur <= floor:
        return x
    gain = target / cur
    peak_now = float(np.abs(x).max())
    if peak_now * gain > peak:
        gain = peak / peak_now
    return (x * gain).astype("float32")


def _pcm_to_wav_b64(pcm_float, sr):
    import numpy as np
    pcm = (np.asarray(pcm_float, dtype="float32").clip(-1.0, 1.0) * 32767.0).astype("<i2")
    buf = io.BytesIO()
    with wave.open(buf, "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(int(sr))
        f.writeframes(pcm.tobytes())
    return base64.b64encode(buf.getvalue()).decode("ascii")


class WorkerState:
    def __init__(self, idle_unload_s=600):
        self.model = None
        self.variant = None
        self.last_used = 0.0
        self.idle_unload_s = idle_unload_s
        self.loader = _load_model
        self.lock = threading.Lock()


def _free_cuda():
    try:
        import torch
        torch.cuda.empty_cache()
    except Exception:
        pass


def maybe_unload(state, now=time.time):
    with state.lock:
        if state.model is not None and (now() - state.last_used) > state.idle_unload_s:
            state.model = None
            state.variant = None
            _free_cuda()


def handle_request(state, req, now=time.time):
    try:
        rtype = req.get("type")
        if rtype == "ping":
            return {"ok": True, "loaded": state.model is not None}
        if rtype == "warm":
            # Load the model ahead of the first synth (pays the cold load now),
            # producing no audio, so the first real digest is fast.
            variant = req.get("variant") or "turbo"
            with state.lock:
                if state.model is None or state.variant != variant:
                    state.model = None
                    _free_cuda()
                    state.model = state.loader(variant)
                    state.variant = variant
                state.last_used = now()
            return {"ok": True, "loaded": True}
        if rtype != "synth":
            return {"ok": False, "error": "unknown request type: {0!r}".format(rtype)}
        variant = req.get("variant") or "turbo"
        with state.lock:
            if state.model is None or state.variant != variant:
                state.model = None
                _free_cuda()
                state.model = state.loader(variant)
                state.variant = variant
            state.last_used = now()
            kwargs = {}
            if req.get("voice_path"):
                kwargs["audio_prompt_path"] = req["voice_path"]
            if req.get("exaggeration") is not None:
                kwargs["exaggeration"] = req["exaggeration"]
            import numpy as np
            # No `or [""]` fallback: an empty/unspeakable text returns zero
            # samples instead of model.generate("") hallucinating audio (#56).
            chunks = _split_text(req.get("text") or "")
            parts = [_to_float_mono(state.model.generate(c, **kwargs)) for c in chunks]
            audio = np.concatenate(parts) if parts else np.zeros(0, dtype="float32")
            audio = _normalize_rms(audio)     # uniform loudness across voices (#81)
            sr = state.model.sr
            state.last_used = now()
        return {"ok": True, "wav_b64": _pcm_to_wav_b64(audio, sr)}
    except Exception as exc:  # noqa: BLE001 - report, never crash the loop
        return {"ok": False, "error": "{0}: {1}".format(type(exc).__name__, exc)}


def _use_clean_stdout():
    """Reserve the real stdout for the JSON protocol and redirect everything else
    to stderr. chatterbox/torch/perth print progress and warnings ("loaded
    PerthNet...", "S3 Token -> Mel Inference...") to sys.stdout during load and
    synth; on the protocol channel the client's line reader mistakes the first
    such line for the response, cannot parse it, and declares the worker dead
    (verified live). Returns the reserved protocol stream to write responses to."""
    proto = sys.stdout
    sys.stdout = sys.stderr
    return proto


def main():
    proto_out = _use_clean_stdout()
    idle = 600.0
    if len(sys.argv) > 1:
        try:
            idle = float(sys.argv[1])
        except ValueError:
            pass
    state = WorkerState(idle_unload_s=idle)

    def _reaper():
        while True:
            time.sleep(30)
            maybe_unload(state)

    threading.Thread(target=_reaper, daemon=True).start()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except ValueError:
            resp = {"ok": False, "error": "bad json"}
        else:
            resp = handle_request(state, req)
        proto_out.write(json.dumps(resp) + "\n")
        proto_out.flush()


if __name__ == "__main__":
    main()
