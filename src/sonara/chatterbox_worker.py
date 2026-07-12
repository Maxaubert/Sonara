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


def _load_model(variant):
    # Verified against docs/superpowers/specs/2026-07-12-chatterbox-smoke.md;
    # adjust there first if the package API changes.
    if variant == "original":
        from chatterbox.tts import ChatterboxTTS
        return ChatterboxTTS.from_pretrained(device="cuda")
    from chatterbox.tts_turbo import ChatterboxTurboTTS
    return ChatterboxTurboTTS.from_pretrained(device="cuda")


def _to_wav_b64(tensor, sr):
    import numpy as np
    pcm = np.asarray(tensor.squeeze().cpu().numpy(), dtype="float32")
    pcm = (pcm.clip(-1.0, 1.0) * 32767.0).astype("<i2")
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
            wav = state.model.generate(req.get("text") or "", **kwargs)
            state.last_used = now()
        return {"ok": True, "wav_b64": _to_wav_b64(wav, state.model.sr)}
    except Exception as exc:  # noqa: BLE001 - report, never crash the loop
        return {"ok": False, "error": "{0}: {1}".format(type(exc).__name__, exc)}


def main():
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
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
