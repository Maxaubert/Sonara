# Chatterbox TTS API (travisvn): sources

- Source: https://github.com/travisvn/chatterbox-tts-api (branch `main`, commit `a5f466128e4b`), fetched 2026-10-06: `app/models/requests.py` (`TTSRequest`, `input` at most 3000 characters), `app/models/responses.py` (`ErrorResponse`, `VoiceLibraryItem`, `VoiceLibraryResponse`), `app/api/endpoints/speech.py` (`POST /audio/speech`, `resolve_voice_path_and_language`), `app/api/endpoints/voices.py` (`GET /voices`), `app/core/voice_library.py` (`get_voice_path`: by name or alias).
- Licence: AGPL-3.0 (the repository). Only interface facts (route paths, field names, types, limits) were transcribed by hand, for interoperability; no code was copied.
- Note: a voice list entry carries both `name` and `filename`; the speech route resolves `name` (or an alias) and silently falls back to the default voice for anything else, such as a file name.
