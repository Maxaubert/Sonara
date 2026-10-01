# Chatterbox streaming playback + responsiveness fixes

Date: 2026-07-12

## Summary

Make Chatterbox synthesis interruptible and gapless by turning it into a
**pipelined, chunk-streamed handle**: `tts.run` returns a handle immediately, and
`handle.wait()` synthesizes chunks ahead on a producer thread while playing the
ready chunks in order. `speaker.cancel()` aborts within one chunk. Also fix two
pause correctness bugs and the transient VRAM-gate notice found in the ecosystem
audit.

## Motivation (from the verified ecosystem audit)

10 integration surfaces are clean. The one real theme: a Chatterbox synth is a
blocking, uninterruptible call on the single speak-loop thread (up to
`chatterbox_timeout=120s`), so nav / mute / pause / skip are captured instantly
but not acted on until the synth returns. Plus two confirmed pause correctness
bugs and a misleading transient-gate notice. Nothing is dangerous (no crashes,
races, or double-speak in normal use); this is responsiveness + polish.

## Part 1: Pipelined streaming Chatterbox handle

### Contract fit

The speaker drives a proc-like handle: `wait(timeout)`, `terminate()`,
`returncode` (0 = completed, non-0/None = interrupted), `poll()`. Today the
chatterbox path synthesizes the whole utterance inside `tts.run` (blocking) and
returns a `_TtsHandle` wrapping one finished WAV. New design: `tts.run` returns a
`_ChatterboxHandle` **before any synthesis**, so:

- `self._current` is set by the speaker the instant `run()` returns, which closes
  the synth-gap for chatterbox (a `cancel()` mid-synth now reaches
  `handle.terminate()` instead of a null `_current`).
- All synthesis happens inside `handle.wait()`.

### The handle (`_ChatterboxHandle` in `platform/windows/tts.py`)

Constructed with: the full text, a `synth_one(chunk) -> wav_bytes` callable
(chatterbox-or-Kokoro-per-chunk, below), an `on_play` callback, and the abort
`threading.Event`.

`wait(timeout)`:
- Split the text into chunks via `chatterbox._split_text` (shared with the worker;
  moved to a place both can import, or duplicated as a tiny pure helper) so the
  daemon drives chunk granularity.
- Start ONE producer thread that, for each chunk in order: if aborted, stop;
  else `wav = synth_one(chunk)`; push `wav` onto a bounded queue
  (`maxsize = 2`, so it synthesizes at most ~1-2 chunks ahead - gapless without
  unbounded VRAM/CPU). Push a `None` sentinel when done or aborted.
- Consumer (the `wait()` body): pop WAVs; fire `on_play` once, before the FIRST
  chunk's playback; play each via the existing `_play_wav_bytes` and wait on the
  returned `_TtsHandle`; between chunks, check the abort Event. Stop on the
  sentinel or on abort.
- `returncode`: 0 if all chunks played to completion; 1 if aborted (so the speaker
  leaves the item unheard for replay, exactly like today).
- Timeout: the speaker's `_wait_timeout` (120s) still bounds the whole handle; each
  worker request uses a shorter per-chunk timeout (`chatterbox_timeout`, lowered
  to 30 - see Part 3) so one wedged chunk cannot approach 120s.

`terminate()`: set the abort Event; stop the currently-playing chunk
(`winsound.PlaySound(None, 0)` via the active sub-handle). Idempotent.

`poll()` / `returncode`: standard.

### Per-chunk synth with fallback (`synth_one`)

`tts.run` decides the engine ONCE up front (kokoro name -> kokoro; native ->
native; chatterbox name -> the streaming path). For the chatterbox streaming path,
`synth_one(chunk)` is a closure that:
- tries `chatterbox.CLIENT.synth_wav(chunk, voice, cfg)` and returns its WAV;
- on `ChatterboxError` (worker died, per-chunk timeout, synth error), synthesizes
  that chunk via Kokoro `DEFAULT_VOICE`, arms the fallback notice
  (`chatterbox._set_fallback_notice`) and logs `[chatterbox] ...` - so audio never
  goes silent mid-utterance and the user hears the rest in af_heart.

The up-front gate/provisioned decision stays in `tts.run`: not provisioned or
`gate_ok(cfg)` False -> synthesize the WHOLE utterance via Kokoro (one
`_play_wav_bytes`, no streaming handle), arm the notice. So the streaming handle is
built only when chatterbox is actually going to be attempted, and per-chunk
fallback covers a mid-stream worker death.

### Worker

The worker keeps its internal `_split_text` as a safety net but now receives one
chunk per `synth` request from the daemon (the daemon drives chunking for
pipelining/abort). No protocol change; `synth_wav` is called once per chunk.

## Part 2: Pause correctness fixes (`daemon.py`)

Verified bugs from the audit:

1. **Pause->resume within one synth drops the in-flight item.** The speak loop's
   requeue guard (`if not completed and self._paused.is_set(): rewind cursor`)
   must correctly re-queue a chatterbox item that was aborted by pause so resume
   re-speaks it, rather than skipping it. Fix: ensure the pause-abort path leaves
   the item unheard AND re-enqueued at the cursor (mirror the existing Kokoro
   pause-requeue, which the audit says works for the non-streaming case - the
   streaming handle returns `returncode != 0` on pause-abort, so the same guard
   applies; verify the cursor-rewind targets THIS item, not the next).

2. **Pause during a "Session changed" announcement double-speaks or loses it.**
   The router's queued announcement (`_pending_announce`) interacts with the
   cursor rewind: pausing while the announcement item is speaking must not rewind
   a real content item. Fix: the announcement item (`kind == "session_change"`,
   `id == 0`, no channel cursor position) must not trigger a channel cursor
   rewind on pause; re-arm the announcement instead (or let it be re-emitted).

Both are covered by daemon tests with a fake speaker that reports `completed=False`
under pause for a streaming-style item.

## Part 3: Transient gate-notice fix

- Lower `chatterbox_timeout` default 120 -> 30 (per-chunk now; a chunk is a few
  seconds, so 30 is generous and bounds a wedged chunk). Config + README.
- A VRAM-gate miss is EXPECTED and transient (ComfyUI busy). It should NOT speak
  "Chatterbox unavailable, using Heart" or burn the once-per-run notice - only a
  genuine failure (not provisioned, worker error/timeout) announces. Split the
  reason: gate-miss -> log only (`[chatterbox] gate: ...`), no
  `_set_fallback_notice`; real failure -> notice + log. So a busy-GPU moment
  quietly uses af_heart and silently returns to Chatterbox when VRAM frees, with
  no spurious announcement.

## Components / files

1. `src/sonara/platform/windows/tts.py` - `_ChatterboxHandle` (new class),
   `run()` chatterbox branch rewritten to build it, `synth_one` closure,
   gate-miss vs failure split.
2. `src/sonara/chatterbox.py` - expose a `split_text` helper (or move the
   worker's `_split_text` to a shared pure module both import); keep
   `_set_fallback_notice` for real failures only.
3. `src/sonara/chatterbox_worker.py` - unchanged protocol; one chunk per request
   (internal `_split_text` retained as a safety net).
4. `src/sonara/daemon.py` - pause requeue fix; announcement-pause fix.
5. `src/sonara/config.py` + README - `chatterbox_timeout` default 30; document
   the streaming/interruptibility behavior and that a busy GPU quietly uses
   af_heart.

## Testing

- `_ChatterboxHandle` with injected fake `synth_one` (returns tiny WAV bytes) and
  a fake `_play_wav_bytes`/sub-handle: chunks play in order; producer synthesizes
  ahead (queue bounded); `on_play` fires once before first playback; `terminate()`
  aborts within one chunk (remaining chunks not synthesized/played);
  `returncode` 0 on complete, 1 on abort; a `synth_one` raising falls back per
  chunk (Kokoro path invoked, notice armed) without going silent.
- `run()` routing: gate-miss / not-provisioned -> whole-utterance Kokoro, no
  streaming handle, no notice for gate-miss; chatterbox chosen -> streaming
  handle built.
- daemon: pause->resume re-speaks the interrupted (streaming) item; pause during a
  session-change announcement does not rewind a content item; mute drops the item
  before building the handle (no synth).
- config: `chatterbox_timeout == 30`.
- No real GPU/torch in CI (all seams injected); one manual live smoke after deploy.

## Out of scope

- Cross-utterance synth caching.
- Changing the speaker or speak-loop contracts (the handle fits them as-is).
- Mid-chunk cancellation (chunk granularity is the interruptibility unit).

## Global constraints

- Daemon stdlib-only, Python 3.9+; torch/chatterbox only in the worker venv.
- Never silent: per-chunk Kokoro fallback + gate-miss quiet fallback.
- No em-dashes in code/docs.
- The handle's producer thread must be joined/stopped on terminate and on normal
  completion (no leaked threads); the abort Event is the single source of truth.
