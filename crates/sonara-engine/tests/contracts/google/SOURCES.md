# Google Cloud Text-to-Speech: sources

- Spec: the discovery document https://texttospeech.googleapis.com/$discovery/rest?version=v1 (revision 20260827), fetched 2026-10-06.
- Licence: Google publishes discovery documents for client generation under the Google APIs Terms of Service; only the fragment below is kept.
- Extracted: methods `text.synthesize` (`POST v1/text:synthesize`, `SynthesizeSpeechRequest`, `AudioConfig.audioEncoding` with `PCM` and `LINEAR16`, `speakingRate` within [0.25, 2.0]) and `voices.list` (`GET v1/voices`, `languageCode`); schemas kept: SynthesizeSpeechRequest, SynthesisInput, VoiceSelectionParams, AudioConfig, SynthesizeSpeechResponse, ListVoicesResponse, Voice.
- Command: `python packaging/contracts/extract_fragment.py discovery texttospeech.json fragment.json text.synthesize voices.list --keep SynthesizeSpeechRequest,SynthesisInput,VoiceSelectionParams,AudioConfig,SynthesizeSpeechResponse,ListVoicesResponse,Voice`
- Derived (marked `x-derived`): `required` from "Required." and ranges from "in the range [a, b]" in the descriptions. The `X-goog-api-key` header is Google's documented alternative to the `key` query parameter (https://cloud.google.com/apis/docs/system-parameters); errors follow `google-rpc`.
