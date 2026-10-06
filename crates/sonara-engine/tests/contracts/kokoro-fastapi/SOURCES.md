# Kokoro-FastAPI: sources

- Source: https://github.com/remsky/Kokoro-FastAPI (branch `master`, commit `c9cfeb262817`), fetched 2026-10-06: `api/src/structures/schemas.py` (`OpenAISpeechRequest`, `Rate` 0.25 to 4.0, `Volume` 0 to 10) and `api/src/routers/openai_compatible.py` (`POST /v1/audio/speech` and its `HTTPException` details `{error, message, type}`, `GET /v1/audio/voices` with its default `{id, name}` shape and the `?legacy=true` string shape).
- Licence: Apache-2.0 (the repository). FastAPI builds the OpenAPI at run time and the repository ships none, so the fragment is hand-transcribed (field names, types, limits).
