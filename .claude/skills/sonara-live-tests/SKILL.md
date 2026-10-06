---
name: sonara-live-tests
description: Use to run Sonara's opt-in live checks on this PC (OneCore voices, Windows audio/media/hotkeys, Credential Manager, Kokoro, external TTS providers) and which env vars each needs. These are --ignored tests, never part of the gates.
---

# Sonara live tests

Opt-in `--ignored` tests that hit real speech, audio, hotkeys or provider APIs. They are not
gates and CI never runs them. Run from the repo root with `~/.cargo/bin` on PATH. Each one
that needs a variable skips when it is unset, so an all-skipped run proves nothing: say which
ones actually ran.

| What | Command |
|---|---|
| OneCore voices | `cargo test -p sonara-engine --test onecore_live -- --ignored` |
| L4 audio, media and hotkeys (plays sound, presses media keys) | `cargo test -p sonara-system --test win_live -- --ignored` |
| Credential Manager round trip | `cargo test -p sonara-engine --test credman_live -- --ignored` |
| One sentence end to end | `cargo run -p sonara-reader --example say -- --engine onecore "Hello."` |
| Kokoro | `SONARA_KOKORO_MODELS=<folder with the two model files> cargo test -p sonara-engine --release --test kokoro_live -- --ignored --nocapture` (first `python packaging/runtime_dlls.py stage target/release`) |
| External engines | `cargo test -p sonara-engine --test external_live -- --ignored --nocapture` |

## External engine variables

| Provider | Variables |
|---|---|
| OpenAI | `OPENAI_API_KEY` + `SONARA_LIVE_OPENAI_VOICE` (no voice list) |
| Kokoro-FastAPI | `SONARA_LIVE_KOKORO_FASTAPI_URL` |
| LocalAI | `SONARA_LIVE_LOCALAI_URL` + `SONARA_LIVE_LOCALAI_MODEL` |
| Speaches | `SONARA_LIVE_SPEACHES_URL` + `SONARA_LIVE_SPEACHES_MODEL` |
| ElevenLabs | `ELEVENLABS_API_KEY` |
| Azure | `AZURE_SPEECH_KEY` + `AZURE_SPEECH_REGION` |
| Google Cloud TTS | `GOOGLE_TTS_API_KEY` |
| Gemini | `GEMINI_API_KEY` |
| Cartesia | `CARTESIA_API_KEY` + `SONARA_LIVE_CARTESIA_MODEL` |
| Deepgram | `DEEPGRAM_API_KEY` |
| Your own program | `SONARA_LIVE_COMMAND` (JSON argv), optional `SONARA_LIVE_COMMAND_OPTIONS` (JSON object) |

Any provider also takes `SONARA_LIVE_<NAME>_MODEL` and `SONARA_LIVE_<NAME>_VOICE`; without them
it uses the provider's first listed model or voice. Never write a model id or voice name into
code, tests or docs (#235): pass them through these variables.

## Rules

- Keys come from env vars only. Never print them, write them to a file or commit them; the
  non-live tests use `sonarad --keys fake` (`<home>\fake-keys.json`), never Credential Manager.
- Paid providers cost money per call: ask the user before a run that uses their keys.
- `win_live` and the say example make sound and touch media sessions: not during a call or
  someone's listening session.
- Legacy Python live checks (until #248): `python -m pytest -m live_windows`.
- Full context (Kokoro staging, G2P bless, earcons): `docs/testing.md`.
