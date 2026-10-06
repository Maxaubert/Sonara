# Speaches: sources

- Spec: `docs/openapi.json` of https://github.com/speaches-ai/speaches (branch `master`, commit `993994f7984b`, info.version 0.8.3), fetched 2026-10-06.
- Licence: MIT (the repository).
- Extracted: `POST /v1/audio/speech` (`CreateSpeechRequestBody`: `model` required, `sample_rate` 8000 to 48000, the 422 `HTTPValidationError`), `GET /v1/models` (`ListModelsResponse`, `Model.task`).
- Command: `python packaging/contracts/extract_fragment.py openapi openapi.json fragment.json "POST /v1/audio/speech" "GET /v1/models"`
- Hand-added: `GET /v1/audio/voices`. The published spec documents `ListModelsResponse` there (a `response_model` HACK noted in `src/speaches/routers/models.py`); the route returns `{"voices": [...], "object": "list"}` of `KokoroModelVoice` (`src/speaches/executors/kokoro.py`: `name`, `language`, `gender`, a computed `id`) or `PiperModelVoice` (`src/speaches/executors/piper.py`: `name`, `language`, a computed `id`), transcribed from those files at the same commit.
