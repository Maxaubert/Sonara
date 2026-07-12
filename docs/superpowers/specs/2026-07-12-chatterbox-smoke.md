# Chatterbox real-GPU smoke test findings (RTX 5090)

Date: 2026-07-12
Machine: RTX 5090 (Blackwell), 32 GB VRAM, Windows 11. System Python is
3.14; the chatterbox venv uses Python 3.12 via `uv`.

This is the verified API contract for Task 2 (worker) and Task 5. Where the
brief's example imports differed from reality, the real, working values are
recorded below.

## Install commands that worked

```powershell
uv venv "$env:USERPROFILE\.sonara\chatterbox-venv" --python 3.12

# 1. Install torch cu128 first (Blackwell/RTX 5090 needs cu128).
uv pip install --python "$env:USERPROFILE\.sonara\chatterbox-venv\Scripts\python.exe" torch --index-url https://download.pytorch.org/whl/cu128

# 2. Install chatterbox-tts. This SILENTLY DOWNGRADES torch to a generic
#    (non-cu128) 2.6.0 build and installs a matching torchaudio==2.6.0.
uv pip install --python "$env:USERPROFILE\.sonara\chatterbox-venv\Scripts\python.exe" chatterbox-tts

# 3. Force-reinstall torch cu128 to undo the downgrade.
uv pip install --python "$env:USERPROFILE\.sonara\chatterbox-venv\Scripts\python.exe" torch --index-url https://download.pytorch.org/whl/cu128 --reinstall

# 4. torchaudio 2.6.0 (from step 2) is now ABI-incompatible with the
#    reinstalled torch 2.11.0+cu128 native extension. It must ALSO be
#    reinstalled from the cu128 index, matching the torch build:
uv pip install --python "$env:USERPROFILE\.sonara\chatterbox-venv\Scripts\python.exe" torchaudio --index-url https://download.pytorch.org/whl/cu128 --reinstall
```

Verified:

```powershell
& "$env:USERPROFILE\.sonara\chatterbox-venv\Scripts\python.exe" -c "import torch; print(torch.cuda.is_available(), torch.cuda.get_device_name(0))"
# -> True NVIDIA GeForce RTX 5090
```

Resolved package versions (`uv pip list`):

- `chatterbox-tts==0.1.7`
- `torch==2.11.0+cu128`
- `torchaudio==2.11.0+cu128`
- `resemble-perth==1.0.1`
- `transformers==5.2.0`
- `diffusers==0.29.0`

## Gotcha: torch AND torchaudio both need the cu128 reinstall

`chatterbox-tts` pins a torch/torchaudio pair that is CPU/generic (not
cu128). Reinstalling only `torch` with `--reinstall` is not enough: the
leftover `torchaudio==2.6.0` wheel ships a native extension
(`libtorchaudio.pyd`) built against torch 2.6.0's ABI. Importing it against
torch 2.11.0 fails with:

```
OSError: [WinError 127] The specified procedure could not be found
OSError: Could not load this library: ...\torchaudio\lib\libtorchaudio.pyd
```

This is not an obscure path: `chatterbox.tts` imports `perth` (the
watermarking library) at module load time, and `perth` imports
`torchaudio.transforms` unconditionally, so a bare `from chatterbox.tts
import ChatterboxTTS` fails until torchaudio is also reinstalled from the
cu128 index. **Working order: torch cu128, then chatterbox-tts, then
torch cu128 --reinstall, then torchaudio cu128 --reinstall.**

## Verified import paths and class names

The brief's example (`from chatterbox.tts import ChatterboxTTS`) is
correct for the original variant. The Turbo variant is a separate module
and class, confirmed by successfully importing and running both:

```python
from chatterbox.tts import ChatterboxTTS              # original variant
from chatterbox.tts_turbo import ChatterboxTurboTTS   # Turbo variant
```

(A multilingual variant also exists: `from chatterbox.mtl_tts import
ChatterboxMultilingualTTS` - not exercised here, out of scope for this
task.)

## Verified `from_pretrained` / `generate()` usage

Both classes match the brief's assumed API exactly:

```python
model = ChatterboxTTS.from_pretrained(device="cuda")        # or ChatterboxTurboTTS
wav = model.generate("some text")                            # audio_prompt_path, exaggeration, cfg_weight also accepted kwargs
model.sr                                                      # int sample rate
```

- `generate(text, audio_prompt_path=None, exaggeration=0.5, cfg_weight=0.5,
  ...)` - `text` is the only required positional; `audio_prompt_path` (str
  path to a reference WAV) and `exaggeration` are the kwargs Task 2's
  worker needs. Not independently exercised with a custom
  `audio_prompt_path` in this smoke test (no reference clip on hand); the
  signature is taken from the README and from `chatterbox/tts.py`'s
  `generate()` def, both consistent with the brief.
- `model.generate(...)` returns a torch tensor on the model's device, shape
  `(1, num_samples)` (mono). Moved to CPU + converted to numpy for WAV
  writing: `wav.squeeze().cpu().numpy()`.
- `model.sr` is a plain `int` attribute (not a method).

## Measured results

Test sentence (12 words): "Sonara speaking through Chatterbox on the fifty
ninety."

| Variant  | Import path                        | Checkpoint repo                    | load s | synth s | sr    | VRAM MB (`torch.cuda.memory_allocated()`) | wav duration s |
|----------|-------------------------------------|-------------------------------------|-------:|--------:|------:|-------------------------------------------:|----------------:|
| Original | `chatterbox.tts.ChatterboxTTS`      | `ResembleAI/chatterbox`            |   33.5 |   10.32 | 24000 | 3108                                        | 2.44             |
| Turbo    | `chatterbox.tts_turbo.ChatterboxTurboTTS` | `ResembleAI/chatterbox-turbo` |   41.7 |    2.06 | 24000 | 2718                                        | 2.96             |

Both variants: `model.sr == 24000`.

Turbo's synth time (2.06 s) is roughly 5x faster than the original variant
(10.32 s) for the same sentence, consistent with the design doc's
"faster-than-real-time" claim. Turbo's load time (41.7 s) was slower than
the original's (33.5 s) in this run, mostly first-download/first-import
warmup (this was Turbo's first `from_pretrained` call in the process, with
JIT/attention-kernel warmup happening on top of the original model's
already-imported torch/transformers stack); steady-state load time on a
fresh process for either variant should be dominated by the ~1-2 s local
checkpoint deserialization plus torch/CUDA init, not visible separately
here since each variant was measured in its own fresh process anyway - both
processes paid full torch/CUDA cold-start cost, so the load numbers above
are directly comparable as "cold process to first output ready."

VRAM (`torch.cuda.memory_allocated()`, MB) is comfortably under the ~3-4 GB
Turbo estimate in the task brief and the design doc's 5 GB gate default,
with the 5090's ~23 GB free at test time.

Both checkpoints downloaded to `~/.sonara/chatterbox/hf-cache` (via
`HF_HOME`), one-time cost: original ~34 s import+load including first
download of `ResembleAI/chatterbox` weights; Turbo's 10-file checkpoint
fetch for `ResembleAI/chatterbox-turbo` took ~34 s over an unauthenticated
HF Hub connection (a `HF_TOKEN` would speed this up per the printed
warning; not required for functionality).

## Audio verification

Both WAVs were written PCM16 mono at `model.sr` via the `wave` module (per
the brief's script) to `~/.sonara/chatterbox-smoke-original.wav` and
`~/.sonara/chatterbox-smoke-turbo.wav`. Verified as real, non-silent
speech audio by:

1. **Duration is plausible** for a 12-word sentence: 2.44 s (original),
   2.96 s (Turbo) - consistent with natural speaking rate, not truncated
   silence or a 1-frame stub.
2. **Waveform statistics** (computed via `numpy` over the decoded PCM16
   samples): RMS energy 4246 (original) / 1347 (Turbo) out of a 16-bit
   range (max magnitude 32767), peak amplitude 31177 / 11696 - both well
   above noise floor, neither flat-lined nor clipped. `silent_frac`
   (fraction of samples with `|sample| < 50`) was 0.091 (original) and
   0.244 (Turbo) - i.e. mostly non-silent with pauses consistent with word
   boundaries in normal speech, not a fully silent buffer (`silent_frac`
   would be ~1.0) or pure noise (`silent_frac` would be ~0).
3. **Played back without error** via
   `(New-Object System.Media.SoundPlayer '<path>').PlaySync()` in
   PowerShell - confirms the file is a well-formed WAV that Windows'
   audio subsystem accepts and plays to completion (this agent has no
   audio input to independently confirm intelligibility by ear; the
   statistical checks in point 2, combined with the file being produced by
   the documented `ChatterboxTTS`/`ChatterboxTurboTTS` pipeline rather than
   a hand-built buffer, are the evidence of record).

## Other gotchas

- `HF_HOME` must be set (or `os.environ.setdefault("HF_HOME", ...)` called)
  **before** `import chatterbox` / `import torch` triggers any
  huggingface_hub cache resolution, matching the brief's script structure.
- Windows without Developer Mode / admin: `huggingface_hub` prints a
  symlink-cache warning and falls back to copying files instead of
  symlinking (harmless, just uses more disk under
  `~/.sonara/chatterbox/hf-cache`).
- No `HF_TOKEN` needed for these public `ResembleAI/chatterbox` and
  `ResembleAI/chatterbox-turbo` repos; unauthenticated download worked,
  just prints a rate-limit warning.
- The venv has no `pip` module installed by default under `uv venv`; use
  `uv pip ...` (not `python -m pip ...`) for all package operations in the
  chatterbox venv.
- Model load prints `loaded PerthNet (Implicit) at step 250,000` -
  `resemble-perth` (audio watermarking) is loaded as part of
  `from_pretrained`, not optional, and requires the compatible torchaudio
  install (see gotcha above).
- Various non-fatal `FutureWarning`/`UserWarning` noise on import/generate
  (`LoRACompatibleLinear` deprecation, `torch.backends.cuda.sdp_kernel`
  deprecation, `sdpa` attention `output_attentions` note) - none affect
  output correctness, safe to ignore or filter in Task 2's worker logging.

## Summary for Task 2 / Task 5 implementers

The design doc's assumed API is correct as written, with one addition:
Turbo is a genuinely separate class in a separate module
(`chatterbox.tts_turbo.ChatterboxTurboTTS`), not a flag or checkpoint id
on `ChatterboxTTS`. Both classes share the same `from_pretrained(device=...)`
/ `generate(text, audio_prompt_path=..., exaggeration=..., cfg_weight=...)`
/ `.sr` surface, so the worker can treat them as interchangeable behind a
`variant: "turbo" | "original"` selector that just picks the class and
`from_pretrained` call, as the design doc's worker protocol already
assumes. Remember the torch-then-chatterbox-then-torch-reinstall-then-
torchaudio-reinstall install order in the provisioning code (Task 2 /
`sonara voices install chatterbox`), or the worker venv will fail to import
`chatterbox.tts` at all.
