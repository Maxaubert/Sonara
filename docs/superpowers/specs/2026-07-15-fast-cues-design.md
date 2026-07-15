# Fast cues: control feedback speaks through the always-warm Windows voice

**Date:** 2026-07-15
**Status:** approved by user (scope: ALL control feedback, recommended option)

## Problem

Every spoken cue ("Muted.", "Paused.", "Summary mode on.", session-change
announcements, option re-reads, fallback notices) goes through `_speak_cue` ->
the CONTROL channel -> the speak loop -> the CONFIGURED voice. With a
Chatterbox voice selected and the worker idle-unloaded, the cue waits out a
multi-second cold model reload; mute/pause feel unresponsive (long-standing
report, see memory note sonara-control-phrase-prerender-idea).

## Design

Route CONTROL-channel items and session-change announcements to the native
WinRT Windows voice, which synthesizes near-instantly with no model load.
`platform/windows/tts.py::run` already maps `voice=None` to "best WinRT
voice", so the override is `voice=None` at the speak call.

- `Speaker.speak` gains a per-call `voice=` override (module sentinel `_UNSET`
  distinguishes "no override" from "override to None"); `self._voice` remains
  the default. `say_runner` signature unchanged.
- Daemon: a `_cue_voice_override(item)` helper returns `{"voice": None}` when
  `config.fast_cues` is truthy AND (`item.session == CONTROL` or
  `item.kind == "session_change"`); `{}` otherwise. Applied at BOTH
  `speaker.speak` item call sites (the paused pause-exempt drain and the main
  loop).
- Config: `fast_cues: true` (default ON). Clamp in `set_config_value`:
  `bool(v)`.
- Webui: `fast_cues` added to `_PAGE_KEYS`/`_CONFIG_KEYS`; settings page gets
  an "Instant cues" switch in Speech > Delivery.
- Dynamic cues (option re-reads, catch-up preambles, duck-level confirmations,
  fallback notices) are CONTROL items too and intentionally included (user
  choice: all control feedback).
- Rate: cues keep the configured wpm; only the voice changes.

## Out of scope

Pre-rendered per-voice cue WAVs (superseded by this approach), a cue-voice
picker, Kokoro-if-warm routing.

## Testing

- speaker: default voice used without override; `voice=None` override reaches
  the say_runner; existing 3-arg runners unaffected.
- daemon (new test file, daemon_helpers): CONTROL cue speaks with voice=None;
  ordinary session prose keeps the configured voice; `fast_cues=false`
  disables the override; session_change announcement gets the override.
- config: default + documented-keys set.
- webui: page contains the switch; /api/set accepts fast_cues.
- Live: cold-Chatterbox mute toggle responds instantly in the Windows voice.
