# Chatterbox TTS engine (GPU worker + reference-clip voices)

Date: 2026-07-12

## Summary

Add the Chatterbox voice family (Resemble AI, MIT) as a third Sonara speech
engine beside Windows-native and Kokoro. Chatterbox runs as a **persistent GPU
worker subprocess** in its own uv-managed Python 3.12 venv (torch does not
support the daemon's Python 3.14). Voices are **10-second reference WAV clips**
in a user folder; the model imitates the clip. Kokoro af_heart remains the
always-available fallback, and a **VRAM load-gate** picks Chatterbox only when
the GPU has room.

## Motivation

Research (2026-07-12) ranked Chatterbox Turbo as the best downloadable upgrade
over Kokoro af_heart: above-ElevenLabs blind-test preference, MIT license,
350M params, faster-than-real-time on the user's RTX 5090. Windows Natural
voices were investigated first and are a dead end (sandboxed to Narrator; no
tokens registered for third-party APIs). Chatterbox also brings unlimited
voices via reference clips, removing the need to integrate more engines.

## Decisions already made (with user)

- Engine first with one built-in default voice; voice curation afterwards
  (pure data: dropping clips in a folder).
- Turbo is the default variant; original Chatterbox available per-voice. Only
  the variant a selected voice needs is loaded (both only if the user's voices
  span both).
- Idle-unload after 10 minutes (configurable) frees VRAM for ComfyUI.
- **VRAM priority gate**: before loading, check free VRAM; below the threshold
  (default 5 GB) speak via Kokoro instead and re-check next utterance. Once
  loaded, no per-utterance re-check (idle-unload governs). Threshold 0 =
  always try.
- Fallback on ANY worker failure: speak the utterance with the Kokoro default
  voice, one spoken notice per daemon run, always a log line - never silence.

## Architecture

```
daemon (py3.14)                      chatterbox worker (py3.12 venv)
  tts.run(text, voice, rate, on_play)
    voice routing:
      kokoro voice   -> in-process kokoro (unchanged)
      chatterbox v.  -> VRAM gate -> ChatterboxClient ----> worker process
      else           -> WinRT native (unchanged)             loads model once
                                                             synthesizes WAV
    <- WAV bytes ------------------------------------------- returns b64 WAV
    on_play() ; _play_wav_bytes(data)        (idle 10 min -> unloads model)
```

- **Worker process**: `<venv>/Scripts/python.exe -m sonara_chatterbox_worker`
  (a single-file worker script shipped in the package and copied at provision
  time; it must not import sonara - the venv has only chatterbox deps).
  Newline-delimited JSON over stdin/stdout, one request at a time (the speak
  loop is serial):
  - `{"type": "synth", "text": ..., "voice_path": <wav path or null>,
     "variant": "turbo"|"original", "exaggeration": <float, optional>}`
    -> `{"ok": true, "wav_b64": ...}` or `{"ok": false, "error": "..."}`
  - `{"type": "ping"}` -> `{"ok": true, "loaded": bool, "variant": ...}`
  - null voice_path = the model's built-in default voice (this is the bundled
    "cb_default" voice - no licensing question, nothing to ship).
  - The worker unloads the model (frees VRAM, empties CUDA cache) after
    `idle_unload_s` without requests; the process stays (cheap) and reloads
    lazily. Daemon kills the worker on shutdown.
- **Client (daemon side)**: `src/sonara/chatterbox.py` - spawn-on-demand,
  request/response with a synthesis timeout (default 120 s), dead-worker
  respawn (once per utterance), all failures -> `ChatterboxError`.
- **VRAM gate**: `nvidia-smi --query-gpu=memory.free` (subprocess, ~50 ms)
  checked only when the model is not yet loaded. Unavailable nvidia-smi ->
  treat as gate-passed (let the worker try; failure falls back anyway).
- **Playback path unchanged**: WAV bytes go through `_play_wav_bytes`;
  cancel/mute/pause/earcons/`on_play` ducking behave identically. Mid-
  synthesis cancellation is not supported (same as Kokoro today).

## Provisioning

`sonara voices install chatterbox` (extends the existing Kokoro voices
command surface):
1. Resolve Python 3.12 (py launcher) - actionable error if absent.
2. `uv venv ~/.sonara/chatterbox-venv --python 3.12`.
3. `uv pip install` torch (cu128 index, Blackwell/RTX 5090 support) +
   `chatterbox-tts`.
4. Copy the worker script into the venv; set `HF_HOME` to
   `~/.sonara/chatterbox/hf-cache` so model weights live under ~/.sonara.
5. Warm-download weights (Turbo by default) and run a one-sentence smoke
   synth; print measured VRAM/latency.
`sonara voices remove chatterbox` deletes venv + cache. `sonara uninstall`
mentions leftovers. Requires network; PRIVACY.md gets a short note (install
downloads models; synthesis is fully local).

## Voices

- Registry: `~/.sonara/voices/chatterbox/<name>.wav` (a ~10 s clean speech
  clip). Voice name = file stem. Optional `<name>.json` sidecar:
  `{"variant": "turbo"|"original", "exaggeration": 0.5}`.
- `cb_default` is always available (no clip; model's built-in voice).
- `sonara voice <name>` and `sonara voice` listing include chatterbox voices
  (marked with the engine). `chatterbox:` prefix accepted like `kokoro:`.
- Routing in `tts.run`: kokoro name -> kokoro; chatterbox name (registry hit
  or `cb_default`/prefix) -> chatterbox; else native.
- Speech rate (wpm) does NOT apply to chatterbox voices in v1 (the model has
  no rate control); documented in README.

## Config

- `chatterbox_variant`: "turbo" (default) | "original" - default for voices
  without a sidecar.
- `chatterbox_min_free_vram_gb`: 5 (0 = always try).
- `chatterbox_idle_unload_s`: 600.
- `chatterbox_timeout`: 120 (seconds per synthesis request).

## Failure handling

Any chatterbox path failure (venv missing, spawn error, gate below threshold,
synth error/timeout, worker died twice) -> synthesize the utterance with the
Kokoro default voice instead. First fallback per daemon run also speaks a
short notice ("Chatterbox unavailable, using Heart."); every fallback logs a
reason line (`[chatterbox] ...`) to speechd.log. Doctor rows: venv present +
importable, worker spawn/ping, nvidia-smi visible, weights cached, registry
folder. The gate path logs but does not speak a notice (expected, frequent).

## Testing

- Worker protocol: unit-test the client against a fake worker (a tiny python
  subprocess echoing canned JSON) - synth ok, error passthrough, timeout,
  dead-worker respawn-once, b64 decode.
- Worker script logic (loaded/unload timer, request dispatch) tested pure
  with a fake model object; no torch in CI.
- Routing: chatterbox names -> chatterbox path; kokoro/native unchanged.
- VRAM gate: fake nvidia-smi output above/below threshold; missing binary.
- Fallback: forced ChatterboxError -> kokoro synth called, notice once, log.
- Registry: file discovery, sidecar parsing, cb_default.
- Provisioning: command assembly with injected runners (no real installs).
- Real-GPU smoke script (manual/plan task 1): provision on the actual 5090,
  measure VRAM + first-audio latency, verify audio plays.

## Out of scope (v1)

- Streaming synthesis (time-to-first-audio optimization) - later.
- Mid-synthesis cancellation (matches Kokoro's existing behavior).
- Voice curation/import UX beyond the folder convention.
- Emotion/exaggeration controls surfaced in CLI (sidecar only).
- Multi-GPU selection.

## Global constraints

- Daemon stays stdlib-only on Python 3.9+; ALL torch/chatterbox deps live in
  the worker venv. The worker script must not import sonara.
- No em-dashes in code comments or docs.
- Never silent: every failure path ends in kokoro fallback + log.
- Windows-only (matches Sonara).
