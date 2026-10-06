# OpenAI: sources

- Spec: `openapi.json` of https://github.com/openai/openai-openapi (branch `main`, commit `451c025c3918`, OpenAPI 3.1, info.version 2.3.0), fetched 2026-10-06 from https://raw.githubusercontent.com/openai/openai-openapi/main/openapi.json
- Licence: MIT (the repository). Only the fragment below is kept.
- Extracted: `POST /audio/speech` (`CreateSpeechRequest`, the `Voice` union with the custom `{"id"}` object, `response_format`, `speed` 0.25 to 4.0, `instructions`, `stream_format`), its documented answers 400, 401, 403, 429, 500, 503 (`ErrorResponse`), `GET /models` (`ListModelsResponse`), and `securitySchemes` (http bearer).
- Command: `python packaging/contracts/extract_fragment.py openapi openapi.json fragment.json "POST /audio/speech" "GET /models"`
- Note: `CreateSpeechRequest` has `additionalProperties: false`, so the `openai` preset must send no server-specific field (`stream`, `sample_rate`).
