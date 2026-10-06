# ElevenLabs: sources

- Spec: https://api.elevenlabs.io/openapi.json (OpenAPI 3.1), fetched 2026-10-06.
- Licence: ElevenLabs' published API description; no licence is stated. Only the schema fragment Sonara's requests and answers touch is kept, for interoperability testing.
- Extracted: `POST /v1/text-to-speech/{voice_id}` and `/stream` (`output_format` enum, `enable_logging`, the `xi-api-key` header parameter, `Body_text_to_speech_full` and `_stream`, `VoiceSettingsResponseModel`, the 422 `HTTPValidationError`), `GET /v2/voices` (`page_size` at most 100, `next_page_token`, `GetVoicesV2ResponseModel`), `GET /v1/models` (`ModelResponseModel`).
- Command: `python packaging/contracts/extract_fragment.py openapi openapi.json fragment.json "POST /v1/text-to-speech/{voice_id}" "POST /v1/text-to-speech/{voice_id}/stream" "GET /v2/voices" "GET /v1/models" --prune SampleResponseModel,FineTuningResponseModel,VoiceSharingResponseModel,VoiceVerificationResponseModel,VerifiedVoiceLanguageResponseModel,PronunciationDictionaryVersionLocatorRequestModel,ModelRatesResponseModel,LanguageResponseModel,VoiceSharingModerationCheckResponseModel`
- Note: the spec documents only 422 as an error answer; the 401, 402, 403, 404 and 429 codes Sonara maps come from ElevenLabs' error docs and are covered by `external_cloud.rs` and the adapter's unit tests.
