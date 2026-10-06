# openedai-speech: sources

- Source: https://github.com/matatonic/openedai-speech (branch `main`, commit `31f033595a81`), fetched 2026-10-06: `speech.py` (`GenerateSpeechRequest`, the pcm content types `audio/pcm;rate=22050` for tts-1 and `audio/pcm;rate=24000` for tts-1-hd) and `openedai.py` (the error body `{message, code, type, param}`).
- Licence: AGPL-3.0 (the repository). Only interface facts (field names, defaults, content types, the error shape) were transcribed by hand, for interoperability; no code was copied.
- Note: the server has no voice list route (voices map through `config/voice_to_speaker.yaml`).
