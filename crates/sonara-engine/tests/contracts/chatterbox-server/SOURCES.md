# Chatterbox-TTS-Server (devnen): sources

- Source: https://github.com/devnen/Chatterbox-TTS-Server (branch `main`, commit `915ae289340e`), fetched 2026-10-06: `server.py` (`OpenAISpeechRequest`, `POST /v1/audio/speech` with its 400, 404 and 503 `HTTPException` details, `GET /get_predefined_voices`) and `utils.py` (`get_predefined_voices`: `[{display_name, filename}]`).
- Licence: MIT (the repository). Hand-transcribed (the OpenAPI is generated at run time).
- Note: `OpenAISpeechRequest.model` is a required `str` with no default, and the route never reads it.
