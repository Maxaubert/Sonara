# Chatterbox Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the Chatterbox TTS family (Turbo default + original) as a third Sonara engine: a persistent GPU worker in its own Python 3.12 uv venv, reference-clip voices in a user folder, a VRAM load-gate, and an always-audible Kokoro fallback.

**Architecture:** The daemon (Python 3.14, stdlib-only) talks newline-JSON over stdin/stdout to a persistent worker process running in `~/.sonara/chatterbox-venv` (torch/cu128 + chatterbox-tts). `tts.run` routes chatterbox voice names through a `ChatterboxClient`; returned WAV bytes flow into the existing `_play_wav_bytes` path so cancel/mute/duck/`on_play` are untouched. Any failure synthesizes via Kokoro instead, with one spoken notice per daemon run and a `[chatterbox]` log line.

**Tech Stack:** Python 3.9+ stdlib daemon side; worker venv: Python 3.12, torch (cu128 index), chatterbox-tts; uv for provisioning (mirrors `kokoro_provision.py`); pytest with injected runners/fake workers (no torch in CI).

## Global Constraints

- The daemon and everything under `src/sonara/` stays stdlib-only on Python 3.9+ EXCEPT `chatterbox_worker.py`, which runs only inside the venv (Python 3.12) and must NOT import `sonara`.
- Worker protocol messages (exact shapes): `{"type": "synth", "text": str, "voice_path": str|null, "variant": "turbo"|"original", "exaggeration": float|null}` -> `{"ok": true, "wav_b64": str}` | `{"ok": false, "error": str}`; `{"type": "ping"}` -> `{"ok": true, "loaded": bool}`.
- Config keys (exact defaults): `chatterbox_variant="turbo"`, `chatterbox_min_free_vram_gb=5`, `chatterbox_idle_unload_s=600`, `chatterbox_timeout=120`.
- Voices: `~/.sonara/voices/chatterbox/<name>.wav` + optional `<name>.json` sidecar `{"variant": ..., "exaggeration": ...}`; `cb_default` = built-in voice (null voice_path). `chatterbox:` prefix accepted like `kokoro:`.
- Never silent: every chatterbox failure path ends in Kokoro fallback + a `[chatterbox] ...` stderr log line; first fallback per daemon run also speaks "Chatterbox unavailable, using Heart."
- VRAM gate only when the model is not yet loaded; nvidia-smi missing/unparseable = gate passes.
- Speech rate (wpm) does not apply to chatterbox voices (documented).
- No em-dashes in code comments or docs. Tests: `./.venv/Scripts/python.exe -m pytest <files> -q` from repo root. The full suite has 19 PRE-EXISTING env-only failures (test_win_tts, test_winfakes, test_transport, test_paths, test_win_autostart, test_bin_sonara, test_daemon_ducking); add no new failures.

---

### Task 1: Real-GPU smoke test + pinned requirements + API findings doc

This task runs REAL commands (network download of several GB, GPU use). Its deliverables are the dependency pins and the verified chatterbox API that Tasks 2 and 5 consume.

**Files:**
- Create: `src/sonara/requirements-chatterbox.txt` (verified pins)
- Create: `docs/superpowers/specs/2026-07-12-chatterbox-smoke.md` (findings)

**Interfaces:**
- Produces: verified package pins; the exact import path + class names + `generate()` signature for Turbo and original; measured VRAM (MB) and first-audio latency on the RTX 5090; the model's output sample rate attribute. Task 2's worker code is written against `ChatterboxTTS.from_pretrained(device=...)` / `ChatterboxTurboTTS.from_pretrained(device=...)` and `model.generate(text, audio_prompt_path=..., exaggeration=...)` returning a torch tensor with `model.sr` - if reality differs, THIS document is where the correct API is recorded, and Task 2's implementer must follow it.

- [ ] **Step 1: Provision the real venv**

Run (PowerShell or bash; uv is at `C:\Users\Admin\AppData\Local\Microsoft\WinGet\Packages\astral-sh.uv_Microsoft.Winget.Source_8wekyb3d8bbwe\uv.exe`, also on PATH as `uv`):

```powershell
uv venv "$env:USERPROFILE\.sonara\chatterbox-venv" --python 3.12
uv pip install --python "$env:USERPROFILE\.sonara\chatterbox-venv\Scripts\python.exe" torch --index-url https://download.pytorch.org/whl/cu128
uv pip install --python "$env:USERPROFILE\.sonara\chatterbox-venv\Scripts\python.exe" chatterbox-tts
```

If `chatterbox-tts` pins a torch version that conflicts with cu128, record the resolution that works (e.g. install chatterbox-tts first, then force-reinstall torch cu128). The GOAL is: `python -c "import torch; print(torch.cuda.is_available(), torch.cuda.get_device_name(0))"` prints `True NVIDIA GeForce RTX 5090`.

- [ ] **Step 2: Verify the API and synthesize with BOTH variants**

Read https://github.com/resemble-ai/chatterbox README for current class names. Then, with `HF_HOME` set to `%USERPROFILE%\.sonara\chatterbox\hf-cache`, run a script in the venv python that:

```python
import os, time
os.environ.setdefault("HF_HOME", os.path.expanduser("~/.sonara/chatterbox/hf-cache"))
import torch
# ADJUST imports to the README if they differ - and RECORD what worked:
from chatterbox.tts import ChatterboxTTS
t0 = time.time()
model = ChatterboxTTS.from_pretrained(device="cuda")
print("load s:", round(time.time() - t0, 1))
t0 = time.time()
wav = model.generate("Sonara speaking through Chatterbox on the fifty ninety.")
print("synth s:", round(time.time() - t0, 2), "sr:", model.sr, "shape:", tuple(wav.shape))
print("vram MB:", torch.cuda.memory_allocated() // 2**20)
import numpy as np, wave
pcm = (wav.squeeze().cpu().numpy().clip(-1, 1) * 32767).astype("<i2")
with wave.open(os.path.expanduser("~/.sonara/chatterbox-smoke.wav"), "wb") as f:
    f.setnchannels(1); f.setsampwidth(2); f.setframerate(model.sr)
    f.writeframes(pcm.tobytes())
```

Repeat for the Turbo variant (per README, e.g. `ChatterboxTurboTTS` or a `from_pretrained` model id). Play the wav (`Start-Process` the file or `winsound`) - it must contain intelligible speech.

- [ ] **Step 3: Write the pins file**

`src/sonara/requirements-chatterbox.txt` - the exact working set, with the torch line's index noted (the provisioner installs torch separately with `--index-url`, so this file holds `chatterbox-tts==<version>` plus any pin overrides discovered; put the torch version in a comment):

```
# Chatterbox worker venv (Python 3.12). torch is installed separately with
# --index-url https://download.pytorch.org/whl/cu128 (RTX 5090 needs cu128);
# verified torch==<VERSION> on 2026-07-12.
chatterbox-tts==<VERSION-THAT-WORKED>
```

- [ ] **Step 4: Write the findings doc**

`docs/superpowers/specs/2026-07-12-chatterbox-smoke.md` with: exact install commands that worked, verified import paths + class names + `generate()` kwargs for both variants, `model.sr` value, measured load time, synth time for a ~12-word sentence, VRAM MB for each variant, and any gotchas (e.g. torch reinstall order). This doc is the API contract for Task 2.

- [ ] **Step 5: Commit**

```bash
git add src/sonara/requirements-chatterbox.txt docs/superpowers/specs/2026-07-12-chatterbox-smoke.md
git commit -m "feat(chatterbox): verified cu128 pins + real-GPU smoke findings (RTX 5090)"
```

---

### Task 2: The worker script

**Files:**
- Create: `src/sonara/chatterbox_worker.py`
- Test: `tests/test_chatterbox_worker.py` (new; drives the handler with a fake model - no torch)

**Interfaces:**
- Consumes: the verified API from `docs/superpowers/specs/2026-07-12-chatterbox-smoke.md` (READ IT FIRST; if class names differ from the code below, follow the doc).
- Produces: `handle_request(state, req) -> dict` and `WorkerState` (importable on any Python for tests); `main()` serving the stdin/stdout protocol with idle unload. Task 3's client spawns `[venv_python, <path to this file>]`.

- [ ] **Step 1: Write the failing tests**

```python
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
```

Run: `./.venv/Scripts/python.exe -m pytest tests/test_chatterbox_worker.py -q` -> FAIL (module missing).

- [ ] **Step 2: Implement the worker**

`src/sonara/chatterbox_worker.py` (module docstring MUST note: runs in the chatterbox venv on Python 3.12; must not import sonara; torch imports are lazy so the repo test venv can import the module):

```python
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
    from chatterbox.tts import ChatterboxTurboTTS
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
```

NOTE for the implementer: the fake-model test calls `generate(text, audio_prompt_path=..., exaggeration=...)` only when those keys are set - matching the kwargs-building above. `handle_request` re-raises nothing; the loader error string must reach `error`.

- [ ] **Step 3: Run tests** -> PASS (8).

- [ ] **Step 4: Commit**

```bash
git add src/sonara/chatterbox_worker.py tests/test_chatterbox_worker.py
git commit -m "feat(chatterbox): stdin/stdout GPU worker with idle unload"
```

---

### Task 3: Daemon-side module: client, registry, VRAM gate, config, paths

**Files:**
- Modify: `src/sonara/paths.py` (add, after `KOKORO_VENV` block):

```python
CHATTERBOX_VENV = SONARA_DIR / "chatterbox-venv"    # opt-in uv venv for Chatterbox
CHATTERBOX_HF_CACHE = SONARA_DIR / "chatterbox" / "hf-cache"
CHATTERBOX_VOICES_DIR = SONARA_DIR / "voices" / "chatterbox"


def chatterbox_venv_python() -> str:
    """Absolute path to the Chatterbox venv's Python (may not exist)."""
    return str(CHATTERBOX_VENV / "Scripts" / "python.exe")
```

- Modify: `src/sonara/config.py` DEFAULTS (after the summary keys):

```python
    "chatterbox_variant": "turbo",        # default variant for voices without a sidecar
    "chatterbox_min_free_vram_gb": 5,     # VRAM gate; 0 = always try
    "chatterbox_idle_unload_s": 600,      # worker frees the model after this idle time
    "chatterbox_timeout": 120,            # seconds per synthesis request
```

- Create: `src/sonara/chatterbox.py`
- Test: `tests/test_chatterbox.py` (new), plus one-line additions to `tests/test_config.py` and `tests/test_paths.py` asserting the new defaults/paths exist.

**Interfaces:**
- Produces (consumed by Task 4's routing and Task 5's CLI/doctor):
  - `is_provisioned() -> bool` (venv python exists)
  - `list_voices() -> list[str]` (`["cb_default"] + sorted clip stems`)
  - `is_chatterbox_voice(name) -> bool` (bare stem, `cb_default`, or `chatterbox:` prefix; case-preserving stems, comparison on the normalized name)
  - `normalize_voice(name) -> str|None` (strips `chatterbox:`)
  - `voice_spec(name, config) -> dict` (`{"voice_path": str|None, "variant": str, "exaggeration": float|None}` from the sidecar with config defaults)
  - `free_vram_gb(run=...) -> float|None` (nvidia-smi; None = unknown)
  - `gate_ok(config, run=...) -> bool` (True when threshold<=0, VRAM unknown, or free >= threshold)
  - `class ChatterboxError(Exception)`
  - `class ChatterboxClient: synth_wav(text, name, config) -> bytes` (spawn on demand, request/response with `chatterbox_timeout`, respawn once on a dead pipe, gate NOT included - the caller gates)
  - `worker_script_path() -> str` (the installed `chatterbox_worker.py` beside this module)
  - `pop_fallback_notice() -> str|None` / `_set_fallback_notice(reason)` (module-level once-per-read event used by Task 4)
- The client is constructed once at module level (`CLIENT = ChatterboxClient()`) so the worker persists across utterances.

- [ ] **Step 1: Write the failing tests**

`tests/test_chatterbox.py` (key cases; the implementer writes them ALL before the module):

```python
import json

from sonara import chatterbox as cb


# --- registry ---------------------------------------------------------------

def _reg(monkeypatch, tmp_path):
    d = tmp_path / "voices"
    d.mkdir()
    monkeypatch.setattr(cb, "CHATTERBOX_VOICES_DIR", d)
    return d


def test_list_voices_always_has_default(monkeypatch, tmp_path):
    _reg(monkeypatch, tmp_path)
    assert cb.list_voices() == ["cb_default"]


def test_list_voices_discovers_clips(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "calm-lady.wav").write_bytes(b"RIFF")
    (d / "deep.wav").write_bytes(b"RIFF")
    assert cb.list_voices() == ["cb_default", "calm-lady", "deep"]


def test_is_chatterbox_voice_forms(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "calm-lady.wav").write_bytes(b"RIFF")
    assert cb.is_chatterbox_voice("cb_default")
    assert cb.is_chatterbox_voice("calm-lady")
    assert cb.is_chatterbox_voice("chatterbox:calm-lady")
    assert not cb.is_chatterbox_voice("af_heart")
    assert not cb.is_chatterbox_voice(None)


def test_voice_spec_reads_sidecar_and_defaults(monkeypatch, tmp_path):
    d = _reg(monkeypatch, tmp_path)
    (d / "deep.wav").write_bytes(b"RIFF")
    (d / "deep.json").write_text(json.dumps(
        {"variant": "original", "exaggeration": 0.8}), encoding="utf-8")
    cfg = {"chatterbox_variant": "turbo"}
    spec = cb.voice_spec("deep", cfg)
    assert spec["variant"] == "original" and spec["exaggeration"] == 0.8
    assert spec["voice_path"].endswith("deep.wav")
    default = cb.voice_spec("cb_default", cfg)
    assert default == {"voice_path": None, "variant": "turbo", "exaggeration": None}


# --- VRAM gate ---------------------------------------------------------------

def test_gate_reads_nvidia_smi():
    run = lambda argv, **kw: "11342\n"
    assert cb.free_vram_gb(run=run) > 11
    assert cb.gate_ok({"chatterbox_min_free_vram_gb": 5}, run=run) is True
    assert cb.gate_ok({"chatterbox_min_free_vram_gb": 12}, run=run) is False


def test_gate_passes_when_smi_missing():
    def boom(argv, **kw):
        raise FileNotFoundError("nvidia-smi")
    assert cb.free_vram_gb(run=boom) is None
    assert cb.gate_ok({"chatterbox_min_free_vram_gb": 5}, run=boom) is True


def test_gate_threshold_zero_always_true():
    assert cb.gate_ok({"chatterbox_min_free_vram_gb": 0},
                      run=lambda a, **k: "1\n") is True


# --- client against a scripted fake worker -----------------------------------

FAKE_WORKER = r'''
import base64, json, sys
for line in sys.stdin:
    req = json.loads(line)
    if req["type"] == "ping":
        print(json.dumps({"ok": True, "loaded": False})); sys.stdout.flush()
    elif req["text"] == "die":
        sys.exit(1)
    elif req["text"] == "fail":
        print(json.dumps({"ok": False, "error": "synthetic"})); sys.stdout.flush()
    else:
        print(json.dumps({"ok": True,
                          "wav_b64": base64.b64encode(b"RIFFfake").decode()}))
        sys.stdout.flush()
'''


def _client(tmp_path, monkeypatch, timeout=5):
    script = tmp_path / "fake_worker.py"
    script.write_text(FAKE_WORKER, encoding="utf-8")
    import sys
    monkeypatch.setattr(cb, "chatterbox_venv_python", lambda: sys.executable)
    monkeypatch.setattr(cb, "worker_script_path", lambda: str(script))
    monkeypatch.setattr(cb, "CHATTERBOX_VOICES_DIR", tmp_path)  # empty registry
    return cb.ChatterboxClient()


def test_client_synth_returns_wav_bytes(tmp_path, monkeypatch):
    c = _client(tmp_path, monkeypatch)
    out = c.synth_wav("hello", "cb_default", {"chatterbox_timeout": 5})
    assert out == b"RIFFfake"


def test_client_error_raises_chatterbox_error(tmp_path, monkeypatch):
    c = _client(tmp_path, monkeypatch)
    import pytest
    with pytest.raises(cb.ChatterboxError):
        c.synth_wav("fail", "cb_default", {"chatterbox_timeout": 5})


def test_client_respawns_once_after_dead_worker(tmp_path, monkeypatch):
    c = _client(tmp_path, monkeypatch)
    import pytest
    with pytest.raises(cb.ChatterboxError):
        c.synth_wav("die", "cb_default", {"chatterbox_timeout": 5})
    # worker died; next call must respawn and succeed
    assert c.synth_wav("hello", "cb_default", {"chatterbox_timeout": 5}) == b"RIFFfake"


def test_fallback_notice_pops_once():
    cb._set_fallback_notice("no vram")
    assert cb.pop_fallback_notice() == "no vram"
    assert cb.pop_fallback_notice() is None
```

Run -> FAIL (module missing).

- [ ] **Step 2: Implement `src/sonara/chatterbox.py`**

Requirements the code must meet (write it in the established house style; ~150 lines):
- Registry helpers read `CHATTERBOX_VOICES_DIR` (module attribute so tests monkeypatch it); `list_voices` returns `["cb_default"]` when the dir is missing.
- `free_vram_gb(run=subprocess.check_output)`: `run(["nvidia-smi", "--query-gpu=memory.free", "--format=csv,noheader,nounits"], text=True)`, first line, MiB -> GiB (`/1024.0`); any exception -> None.
- `gate_ok(config, run=...)`: threshold from `chatterbox_min_free_vram_gb` (default 5); `<= 0` -> True; `free is None` -> True; else `free >= threshold`.
- `ChatterboxClient`: `_proc` lazily spawned via `subprocess.Popen([chatterbox_venv_python(), worker_script_path(), str(idle_s)], stdin=PIPE, stdout=PIPE, stderr=DEVNULL, env={**os.environ, "HF_HOME": str(CHATTERBOX_HF_CACHE)}, creationflags=CREATE_NO_WINDOW on nt, text=True, encoding="utf-8")`. `_request(payload, timeout)`: write one JSON line, then read one line with a `threading` timer guard (reading with a timeout on Windows pipes: run `readline()` on a helper thread joined with `timeout`; on timeout kill the proc and raise `ChatterboxError("timeout")`). Dead pipe (write fails / empty read) -> kill, respawn once, retry the request; second failure raises.
- `synth_wav(text, name, config)`: build spec via `voice_spec`, send synth request with `chatterbox_timeout` (default 120), non-`ok` -> `ChatterboxError(resp["error"])`, else `base64.b64decode`.
- `worker_script_path()`: `os.path.join(os.path.dirname(os.path.abspath(__file__)), "chatterbox_worker.py")`.
- Module-level `CLIENT = ChatterboxClient()`, `_FALLBACK: list` + `_set_fallback_notice` / `pop_fallback_notice`.
- `is_provisioned()`: `os.path.exists(chatterbox_venv_python())` (import the path helper from `sonara.paths` but expose module-level names `chatterbox_venv_python` / `CHATTERBOX_VOICES_DIR` / `CHATTERBOX_HF_CACHE` so tests can monkeypatch `cb.<name>`).

- [ ] **Step 3: Add the config/paths assertions**

`tests/test_config.py`:

```python
def test_chatterbox_defaults():
    from sonara.config import DEFAULTS
    assert DEFAULTS["chatterbox_variant"] == "turbo"
    assert DEFAULTS["chatterbox_min_free_vram_gb"] == 5
    assert DEFAULTS["chatterbox_idle_unload_s"] == 600
    assert DEFAULTS["chatterbox_timeout"] == 120
```

`tests/test_paths.py`:

```python
def test_chatterbox_paths_are_under_sonara_dir():
    from sonara import paths
    assert str(paths.CHATTERBOX_VENV).startswith(str(paths.SONARA_DIR))
    assert str(paths.CHATTERBOX_VOICES_DIR).startswith(str(paths.SONARA_DIR))
    assert paths.chatterbox_venv_python().endswith("python.exe")
```

- [ ] **Step 4: Run tests** -> PASS: `./.venv/Scripts/python.exe -m pytest tests/test_chatterbox.py tests/test_config.py tests/test_paths.py -q` (note: test_paths has pre-existing env failures on this machine - only the NEW test must pass; run it directly with `::test_chatterbox_paths_are_under_sonara_dir` to confirm).

- [ ] **Step 5: Commit**

```bash
git add src/sonara/chatterbox.py src/sonara/paths.py src/sonara/config.py tests/test_chatterbox.py tests/test_config.py tests/test_paths.py
git commit -m "feat(chatterbox): client, voice registry, VRAM gate, config"
```

---

### Task 4: Routing + fallback in tts.run, voice listing, spoken notice

**Files:**
- Modify: `src/sonara/platform/windows/tts.py` - `run()` (add the chatterbox branch) and `list_voices()` (advertise chatterbox voices)
- Modify: `src/sonara/daemon.py` - speak-loop notice check
- Test: `tests/test_chatterbox_routing.py` (new)

**Interfaces:**
- Consumes: Task 3's `chatterbox` module (`is_chatterbox_voice`, `normalize_voice`, `is_provisioned`, `gate_ok`, `CLIENT.synth_wav`, `ChatterboxError`, `_set_fallback_notice`, `pop_fallback_notice`, `list_voices`).
- Produces: chatterbox voice names speak through the worker; ANY failure produces Kokoro-default audio instead plus a notice event the daemon speaks once per run.

- [ ] **Step 1: Write the failing tests**

`tests/test_chatterbox_routing.py` - drive `TtsBackend.run` with monkeypatched `sonara.chatterbox` seams and a monkeypatched `_play_wav_bytes` capture; a fake kokoro engine for the fallback path (monkeypatch `WinTts._get_kokoro` like existing kokoro tests do - check `tests/test_win_tts_kokoro.py` for the exact fixture pattern and mirror it):

```python
def test_chatterbox_voice_routes_to_worker(...):
    # voice "calm-lady", gate ok, CLIENT.synth_wav returns b"RIFF..." ->
    # _play_wav_bytes receives exactly those bytes; kokoro NOT called;
    # on_play fired before playback.

def test_gate_failure_falls_back_to_kokoro(...):
    # gate_ok -> False: kokoro synthesizes DEFAULT_VOICE, a fallback notice is set,
    # CLIENT.synth_wav never called.

def test_worker_error_falls_back_to_kokoro(...):
    # CLIENT.synth_wav raises ChatterboxError -> kokoro audio + notice set.

def test_not_provisioned_falls_back(...):
    # is_provisioned False -> kokoro + notice, no spawn attempt.

def test_list_voices_includes_chatterbox_when_provisioned(...):
    # monkeypatch chatterbox.is_provisioned True + registry ["cb_default","x"] ->
    # names present in TtsBackend.list_voices()

def test_daemon_speaks_fallback_notice_once(...):
    # daemon test (make_daemon): seed chatterbox._set_fallback_notice("x"),
    # run _speak_loop_once twice -> exactly one "Chatterbox unavailable" cue
    # enqueued on CONTROL (use the FakeSpeaker/CONTROL channel to assert).
```

Write these as REAL tests with the fixture pattern found in `tests/test_win_tts_kokoro.py`; run -> FAIL.

- [ ] **Step 2: Implement the routing**

In `tts.run`, insert BEFORE the kokoro check (chatterbox names are user-defined stems, so route on the registry test first; kokoro names are fixed and cannot collide with `cb_default`/prefix, but a user clip named `af_heart.wav` must NOT shadow kokoro - so order: kokoro first, then chatterbox; document that clip names colliding with kokoro voices are ignored by routing):

```python
        from sonara import chatterbox
        if (not kokoro.is_kokoro_voice(voice)) and chatterbox.is_chatterbox_voice(voice):
            data = self._chatterbox_or_fallback(text, voice, rate)
            if on_play is not None:
                try:
                    on_play()
                except Exception:  # noqa: BLE001 - ducking must never block speech
                    pass
            return _play_wav_bytes(data)
```

and the helper on the backend class:

```python
    def _chatterbox_or_fallback(self, text: str, voice, rate: int) -> bytes:
        """Chatterbox WAV bytes, or Kokoro-default WAV bytes on ANY failure.
        Never raises for chatterbox reasons; never silent. Speech rate does not
        apply to chatterbox (the model has no rate control)."""
        import sys
        from sonara import chatterbox, kokoro
        from sonara.config import load_config
        cfg = load_config()
        reason = None
        if not chatterbox.is_provisioned():
            reason = "not provisioned (run: sonara voices install chatterbox)"
        elif not chatterbox.gate_ok(cfg):
            reason = "VRAM below threshold; using Kokoro this utterance"
        else:
            try:
                return chatterbox.CLIENT.synth_wav(text, voice, cfg)
            except chatterbox.ChatterboxError as exc:
                reason = str(exc)
        print("[chatterbox] fallback: {0}".format(reason), file=sys.stderr, flush=True)
        chatterbox._set_fallback_notice(reason)
        kokoro.require_installed()
        return self._get_kokoro().wav_bytes(
            text, kokoro.DEFAULT_VOICE, kokoro.rate_to_speed(rate))
```

In `list_voices`, append after the kokoro block:

```python
        from sonara import chatterbox
        cb_voices = chatterbox.list_voices() if chatterbox.is_provisioned() else []
        return native + kokoro_voices + cb_voices
```

In `daemon.py` `_speak_loop_once`, right after the `item = self.router.next_item()` claim block (outside the lock, before the `if item is None` return - anchor on the actual code), add:

```python
        # Chatterbox fallback notice: spoken once per daemon run so an eyes-free
        # user knows WHY the voice changed (the reason is already in the log).
        self._maybe_announce_chatterbox_fallback()
```

with:

```python
    def _maybe_announce_chatterbox_fallback(self) -> None:
        if getattr(self, "_cb_fallback_announced", False):
            return
        try:
            from sonara import chatterbox
            reason = chatterbox.pop_fallback_notice()
        except Exception:  # noqa: BLE001
            return
        if reason:
            self._cb_fallback_announced = True
            self._speak_cue(None, "Chatterbox unavailable, using Heart.",
                            exempt_mute=True)
```

- [ ] **Step 3: Run tests** -> PASS, plus regressions: `./.venv/Scripts/python.exe -m pytest tests/test_win_tts_kokoro.py tests/test_daemon_loop.py tests/test_daemon_summary_mode.py -q`.

- [ ] **Step 4: Commit**

```bash
git add src/sonara/platform/windows/tts.py src/sonara/daemon.py tests/test_chatterbox_routing.py
git commit -m "feat(chatterbox): voice routing with kokoro fallback + once-per-run notice"
```

---

### Task 5: Provisioning + CLI + doctor

**Files:**
- Create: `src/sonara/chatterbox_provision.py`
- Modify: `src/sonara/cli.py` (voices subcommand grows an engine arg; doctor rows)
- Test: `tests/test_chatterbox_provision.py` (new), `tests/test_cli_voices.py` (extend), `tests/test_cli_doctor.py` (extend)

**Interfaces:**
- Consumes: `kokoro_provision.ensure_uv` (reused as-is), `paths.CHATTERBOX_VENV/chatterbox_venv_python/CHATTERBOX_HF_CACHE`, `chatterbox.worker_script_path`.
- Produces: `install_chatterbox(*, ensure_uv=..., run=...)`, `uninstall_chatterbox(rmtree=...)`, `chatterbox_requirements_path()`, `warmup(run=...)`; CLI `sonara voices install [kokoro|chatterbox]` / `sonara voices uninstall [kokoro|chatterbox]` (default kokoro, backward compatible); doctor rows `chatterbox venv`, `chatterbox worker`.

- [ ] **Step 1: Write the failing tests**

`tests/test_chatterbox_provision.py` (mirror `tests/test_kokoro_provision.py`'s injected-runner style - read it first):

```python
def test_provision_creates_venv_and_installs(...):
    # collects run() argvs: uv venv <CHATTERBOX_VENV> --python 3.12;
    # uv pip install torch --index-url .../cu128; uv pip install -r requirements-chatterbox.txt
def test_warmup_runs_worker_ping(...):
    # warmup spawns [venv_python, worker_script, ...] with HF_HOME in env (injected run)
def test_uninstall_removes_venv_and_cache(...):
    # rmtree called for CHATTERBOX_VENV and CHATTERBOX_HF_CACHE when present
def test_install_aborts_cleanly_on_failure(...):
    # provision raising -> uninstall path invoked by the CLI wrapper (test at CLI level)
```

`tests/test_cli_voices.py` additions:

```python
def test_voices_install_chatterbox_dispatches(...):
    # cli.main(["voices", "install", "chatterbox"]) calls chatterbox_provision.install_chatterbox
def test_voices_install_default_still_kokoro(...):
    # cli.main(["voices", "install"]) keeps calling kokoro_provision.install_kokoro
```

`tests/test_cli_doctor.py` additions:

```python
def test_doctor_chatterbox_rows_absent_ok(...):
    # not provisioned -> ("chatterbox", True, "not installed (optional)")
def test_doctor_chatterbox_provisioned_checks_python(...):
    # provisioned (fake venv python exists) -> row reports the venv path
```

Run -> FAIL.

- [ ] **Step 2: Implement**

`src/sonara/chatterbox_provision.py`:

```python
"""Provision the opt-in Chatterbox GPU voice environment.

Chatterbox needs torch (Python <= 3.12; cu128 wheels for Blackwell GPUs like
the RTX 5090), which the system Python 3.14 cannot run. A uv-managed venv at
paths.CHATTERBOX_VENV holds the stack; "provisioned" is derived from the venv
python's existence. All subprocess work goes through injected callables so the
logic is unit-testable (mirrors kokoro_provision).
"""
from __future__ import annotations

import os
import shutil
import subprocess

from sonara import paths
from sonara.kokoro_provision import ensure_uv

_TORCH_INDEX = "https://download.pytorch.org/whl/cu128"


def chatterbox_requirements_path() -> str:
    return os.path.join(os.path.dirname(os.path.abspath(__file__)),
                        "requirements-chatterbox.txt")


def provision(uv: str, run=subprocess.check_call) -> None:
    venv = str(paths.CHATTERBOX_VENV)
    py = paths.chatterbox_venv_python()
    run([uv, "venv", venv, "--python", "3.12"])
    run([uv, "pip", "install", "--python", py, "torch",
         "--index-url", _TORCH_INDEX])
    run([uv, "pip", "install", "--python", py,
         "-r", chatterbox_requirements_path()])


def warmup(run=subprocess.check_call) -> None:
    """Load the model once (downloads weights into HF_HOME) so the first real
    utterance does not stall for minutes."""
    from sonara.chatterbox import worker_script_path
    env = dict(os.environ, HF_HOME=str(paths.CHATTERBOX_HF_CACHE))
    code = ("import json, sys, chatterbox_worker as w; "
            "s = w.WorkerState(); "
            "print(json.dumps(w.handle_request(s, {'type': 'synth', "
            "'text': 'Chatterbox ready.', 'voice_path': None, "
            "'variant': 'turbo', 'exaggeration': None}))[:80])")
    run([paths.chatterbox_venv_python(), "-c", code],
        env=dict(env, PYTHONPATH=os.path.dirname(worker_script_path())))


def install_chatterbox(*, ensure_uv=ensure_uv, provision=provision,
                       warmup=warmup) -> None:
    uv = ensure_uv()
    provision(uv)
    warmup()


def uninstall_chatterbox(rmtree=shutil.rmtree) -> None:
    for p in (paths.CHATTERBOX_VENV, paths.CHATTERBOX_HF_CACHE):
        if os.path.isdir(str(p)):
            rmtree(str(p))
```

CLI: give the existing `voices` subparsers an optional positional `engine` (`choices=["kokoro", "chatterbox"]`, `default="kokoro"`, `nargs="?"`); `_cmd_voices_install`/`_cmd_voices_uninstall` branch on it (chatterbox path prints a size warning, calls `install_chatterbox`, reverts with `uninstall_chatterbox` on failure, and does NOT call `install()` - the daemon needs no rewiring for chatterbox). Doctor: after the `neural voices` block add a `chatterbox` row: not provisioned -> `(True, "not installed (optional)")`; provisioned -> check `os.path.exists(chatterbox_venv_python())` and the voices dir, report paths.

- [ ] **Step 3: Run tests** -> PASS + `./.venv/Scripts/python.exe -m pytest tests/test_kokoro_provision.py tests/test_cli_voices.py tests/test_cli_doctor.py tests/test_chatterbox_provision.py -q`.

- [ ] **Step 4: Commit**

```bash
git add src/sonara/chatterbox_provision.py src/sonara/cli.py tests/test_chatterbox_provision.py tests/test_cli_voices.py tests/test_cli_doctor.py
git commit -m "feat(chatterbox): uv provisioning, voices install chatterbox, doctor rows"
```

---

### Task 6: Docs

**Files:**
- Modify: `README.md`, `PRIVACY.md`, `commands/voices.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: README**

Add a `## Chatterbox voices (optional, GPU)` section after the Enhanced-voice section: what it is (Resemble AI Chatterbox, MIT), one-time `sonara voices install chatterbox` (multi-GB download, NVIDIA GPU required), voices = 10-second clips in `~/.sonara/voices/chatterbox/` (name = filename; `cb_default` built-in), `sonara voice <name>` to select, the VRAM gate + Kokoro fallback behavior (and its config keys), the 10-minute idle unload, and the limitation that the wpm rate setting does not affect chatterbox voices. Update the `/sonara:voices` row in the command table to mention both engines. No em-dashes.

- [ ] **Step 2: PRIVACY.md**

Extend the model-download note: installing chatterbox voices downloads model weights from Hugging Face at install time; synthesis afterwards is fully local; nothing you type or hear leaves the machine.

- [ ] **Step 3: commands/voices.md**

Update the description/argument-hint to `install|uninstall [kokoro|chatterbox]`.

- [ ] **Step 4: Verify + commit**

`git grep -n "chatterbox" README.md PRIVACY.md commands/voices.md` shows the additions; scan added text for em-dashes (none).

```bash
git add README.md PRIVACY.md commands/voices.md
git commit -m "docs: chatterbox voices section + privacy note"
```

---

## Self-Review

**Spec coverage:** worker+venv (T1/T2), protocol shapes (T2, verbatim in Global Constraints), client/timeout/respawn-once (T3), registry+sidecar+cb_default (T3), VRAM gate incl. missing-smi and 0-threshold (T3), routing+fallback+never-silent+once-per-run notice (T4), rate-not-applied documented (T4 helper docstring + T6), provisioning/CLI/doctor (T5), README/PRIVACY (T6), real-5090 smoke first (T1). ✓

**Placeholder scan:** Task 1 Step 3 contains `<VERSION-THAT-WORKED>` intentionally - it is the deliverable of that step's real run, not an unknown; Task 4 Step 1 lists test intents with an explicit instruction to mirror the concrete fixture pattern from `tests/test_win_tts_kokoro.py` (the implementer reads that file; signatures they must hit are fully specified in Step 2's code). Everything else is complete code. ✓

**Type consistency:** protocol field names match between T2 worker, T3 client tests (FAKE_WORKER), and Global Constraints; `voice_spec` dict keys match the worker request builder; `chatterbox_venv_python`/`worker_script_path`/`CHATTERBOX_*` names consistent across T3/T4/T5; `ChatterboxError`, `CLIENT`, `pop_fallback_notice` consistent between T3 and T4. ✓
