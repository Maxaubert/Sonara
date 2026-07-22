# Speech Volume - Design

Date: 2026-07-20
Status: proposed

## Goal

A user-adjustable speech volume for Sonara's own output: quieter AND louder
than the current baseline, because other apps' loudness varies and Sonara
should be tunable against them.

## Approach

`winsound` (the playback backend) has no volume API, and Windows per-app
session volume can only attenuate (100% is the ceiling, and Sonara is already
there). The only mechanism that gives BOTH directions is digital gain: scale
the PCM samples of every synthesized WAV before playback.

Every playback path already funnels through one choke point,
`_play_wav_bytes` in `platform/windows/tts.py`: the native WinRT engine, the
whole-utterance Kokoro path, each Chatterbox chunk (`_ChatterboxHandle`
defaults its `play` to `_play_wav_bytes`), and all spoken control cues. One
gain application covers every engine and every cue.

## Components

### 1. Gain in `tts.py`

- `_scale_wav(data: bytes, percent: int) -> bytes`: a pure stdlib function
  (`wave` + `array`, no `audioop`, which left the stdlib in 3.13). Parses the
  WAV; for 16-bit PCM it multiplies every sample by `percent/100` and clamps
  to int16 (hard-clip protection when boosting); any other sample width
  returns the data unchanged (safe pass-through). `percent == 100` returns
  the input untouched (zero-cost default).
- Module-level volume state: `set_volume(percent)` / `get_volume()`, applied
  inside `_play_wav_bytes`. The earcon helper process is untouched.

### 2. Config + protocol + CLI

- Config key `volume`, default `100`, clamped to 25..200 (percent). Above
  200 the clamp distortion outweighs the loudness; below 25 it is
  effectively muted and mute already exists.
- New `SET_VOLUME` message; daemon handler clamps, persists via
  `save_config`, applies `set_volume` on the platform tts module, and speaks
  a short confirmation cue ("Volume 150 percent.") which itself plays at the
  new volume, so adjusting is audible feedback.
- Daemon startup applies the persisted volume once.
- No CLI subcommand (user decision): the settings page is the only
  user-facing surface; the protocol op exists solely for the page.

### 3. Settings UI

- "Speech volume" slider on the Audio page (top, above the ducking
  controls): range 25-200, step 5, output shows "N %", default 100. Wired
  through `_PAGE_KEYS`/`_MSG_KEYS` exactly like `duck_level`.

## Scope decisions (flagged)

- Earcons (beeps) keep their current loudness in v1; the slider governs
  speech only. A separate earcon volume can follow if wanted.
- Voice previews in the settings page play in the browser and are not
  affected by the daemon-side gain.
- Global, not per-session (the Sessions tab governs per-session behavior;
  volume is an output-device concern).

## Error handling

- Malformed/short WAV data in `_scale_wav`: return input unchanged (playback
  fallback preserved; never raise).
- Junk `SET_VOLUME` values: clamp ints, ignore non-ints.

## Testing

- Pure `_scale_wav` tests (cross-platform, no winsound): identity at 100,
  halving at 50, doubling at 200 with clamp at int16 bounds, pass-through
  for 8-bit/float WAVs and malformed bytes.
- Daemon handler tests: clamp + persist + platform apply (monkeypatched) +
  confirmation cue.
- Webui: `volume` in state and settable via `/api/set`.
- Live ear test: default unchanged, 50 audibly quieter, 200 audibly louder.
