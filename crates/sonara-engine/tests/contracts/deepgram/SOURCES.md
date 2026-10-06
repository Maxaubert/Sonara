# Deepgram: sources

- Spec: `openapi.yml` of https://github.com/deepgram/deepgram-api-specs (branch `main`, commit `3452b586dd59`, OpenAPI 3.1), fetched 2026-10-06 from https://raw.githubusercontent.com/deepgram/deepgram-api-specs/main/openapi.yml
- Licence: CC-BY-4.0 (the repository). Fragment excerpted with attribution to Deepgram.
- Extracted: `POST /v1/speak` (query `model`, `encoding`, `container`, `sample_rate` per encoding, `speed` 0.7 to 1.5, body `SpeakV1Request`, the 400 `ErrorResponse` in its three shapes: text, legacy `err_code`/`err_msg`, modern `category`/`message`/`details`), `GET /v1/models` (`ListModelsV1Response`, the `tts` list), `securitySchemes` (`Authorization: Token <API_KEY>`).
- Command: `python packaging/contracts/extract_fragment.py openapi openapi.yml fragment.json "POST /v1/speak" "GET /v1/models" --prune ListModelsV1ResponseSttModels,ListModelsV1ResponseTtsModelsMetadata`
- Notes: the spec's per-encoding variants overlap (8000 Hz is valid for linear16, mulaw and alaw), so the tests check the linear16 variants by name. The spec documents the 200 answer as an empty JSON object; the audio is the body (`container=none`: raw PCM).
