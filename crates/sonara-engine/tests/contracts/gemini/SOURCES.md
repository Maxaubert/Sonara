# Gemini API: sources

- Spec: the discovery document https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta (revision 20261005), fetched 2026-10-06.
- Licence: Google publishes discovery documents for client generation under the Google APIs Terms of Service; only the fragment below is kept.
- Extracted: methods `models.generateContent`, `models.streamGenerateContent`, `models.list`; schemas kept: GenerateContentRequest, Content, Part, Blob, SpeechMetadata, GenerationConfig, SpeechConfig, VoiceConfig, PrebuiltVoiceConfig, ResponseFormatConfig, AudioResponseFormat, GenerateContentResponse, Candidate, PromptFeedback, ListModelsResponse, Model.
- Command: `python packaging/contracts/extract_fragment.py discovery generativelanguage.json fragment.json models.generateContent models.streamGenerateContent models.list --keep GenerateContentRequest,Content,Part,Blob,SpeechMetadata,GenerationConfig,SpeechConfig,VoiceConfig,PrebuiltVoiceConfig,ResponseFormatConfig,AudioResponseFormat,GenerateContentResponse,Candidate,PromptFeedback,ListModelsResponse,Model`
- Derived (marked `x-derived`): `required` from "Required."; `GenerateContentRequest.model` taken out of it by hand, since in REST the model is the path parameter (`v1beta/{+model}:generateContent`).
- Hand-added: `voices.list` (`GET v1beta/voices`, `page_size`, `page_token`, `next_page_token`, the voice fields), which the discovery document does not carry, from the API reference https://ai.google.dev/api/voices (fetched 2026-10-06).
- Outside the discovery document: `?alt=sse` for streaming (https://ai.google.dev/api/generate-content; the document's `alt` enum lists json, media and proto) and the `x-goog-api-key` header (https://ai.google.dev/gemini-api/docs/api-key). Errors follow `google-rpc`.
