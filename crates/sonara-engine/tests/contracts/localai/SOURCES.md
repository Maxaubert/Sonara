# LocalAI: sources

- Spec: `swagger/swagger.json` of https://github.com/mudler/LocalAI (branch `master`, commit `e34847123e2f`, Swagger 2.0), fetched 2026-10-06.
- Licence: MIT (the repository).
- Extracted: `POST /v1/audio/speech` (`schema.TTSRequest`), `GET /v1/audio/voices` (query `model`, `localai.TTSVoicesResponse`, the 404 `schema.ErrorResponse`), `GET /v1/models` (`schema.ModelsDataResponse`). Auth: `securityDefinitions.BearerAuth` (the `Authorization` header).
- Command: `python packaging/contracts/extract_fragment.py openapi swagger.json fragment.json "POST /v1/audio/speech" "GET /v1/audio/voices" "GET /v1/models"`
