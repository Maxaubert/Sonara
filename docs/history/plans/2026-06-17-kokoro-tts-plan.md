# Kokoro TTS Engine - Implementation Plan

**Goal:** Add Kokoro-82M neural voices to Sonari as a first-class TTS engine, all 28
voices available, selectable live by voice name (`sonari voice af_heart`).

**Architecture:** Portable Kokoro engine (`src/sonari/kokoro.py`) synthesizes to a
WAV; each platform TTS backend plays Kokoro voices through its existing WAV path
(Windows: winsound `_TtsHandle`). Routing is by voice name - a voice in the Kokoro
set goes to Kokoro, anything else to the native engine. Lazy imports + optional
`[kokoro]` extra so base Sonari runs without the ML deps.

**Tech:** kokoro-onnx (onnxruntime), numpy; one ~310 MB model + ~6 MB voices.bin
covers all 28 voices.

---

### Task 1: Kokoro engine module (`src/sonari/kokoro.py`)
- `VOICES` (28 names incl. `af_heart`, `af_nicole`), `is_kokoro_voice`, `normalize_voice`
  (strip `kokoro:`), `rate_to_speed(rate)` (200→1.0, clamp 0.5–2.0).
- `to_wav_bytes(audio_f32, sr)` → 16-bit PCM mono WAV via stdlib `wave`.
- `KokoroEngine(model_dir, factory=None)`: lazy download (model + voices.bin to
  `~/.sonari/kokoro/`) + load; `synth(text, voice, speed)` → `(audio, sr)`;
  `wav_bytes(text, voice, speed)`.
- Test `tests/test_kokoro.py` (mock the Kokoro factory; no real model load).

### Task 2: Windows playback reuse + routing
- `windows/tts.py`: extract `_play_wav_bytes(data) -> _TtsHandle` (mkstemp + PlaySound).
- `run(text, voice, rate)`: if `kokoro.is_kokoro_voice(voice)` → KokoroEngine.wav_bytes
  → `_play_wav_bytes`; else the WinRT path. Lazy KokoroEngine on the backend.
- `list_voices()` appends the Kokoro voices; `best_voice()` unchanged (native default).
- Tests for the routing (mock kokoro engine + winsound).

### Task 3: Wiring + CLI
- `sonari voice` lists native + Kokoro voices; `sonari voice af_heart` sets it.
- Confirm `SET_VOICE` + Speaker.set_voice route live (no engine re-init needed -
  routing is per-utterance in run()).

### Task 4: Packaging
- `pyproject.toml`: `[project.optional-dependencies] kokoro = [...]`.

### Task 5: Personal activation (side actions, not feature code)
- Seed `~/.sonari/kokoro/` from the existing Dialogue-reader copy (no download wait).
- Set the user's config `voice = af_heart`; restart the daemon.

### Task 6: Backlog issue on nimkimi/sonari.
