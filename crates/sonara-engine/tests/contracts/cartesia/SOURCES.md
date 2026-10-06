# Cartesia: sources

- Sources, fetched 2026-10-06:
  - The types Cartesia's Python SDK generates from Cartesia's OpenAPI ("File generated from our OpenAPI spec by Stainless"): https://github.com/cartesia-ai/cartesia-python (branch `main`, commit `0c4825851017`), `src/cartesia/types/tts_generate_params.py`, `raw_output_format_param.py`, `raw_encoding.py`, `generation_config_param.py` (speed 0.6 to 1.5), `voice_specifier_param.py`, `voice_list_params.py` (limit 1 to 100, `starting_after`), `voice.py`, `voice_locale.py`, `pagination.py` (the next cursor is the last voice id), `_client.py` (`Authorization: Bearer`, `cartesia-version: 2026-08-14`, the error status codes).
  - The API reference for version 2026-08-14: https://docs.cartesia.ai/api-reference/tts/bytes and https://docs.cartesia.ai/api-reference/voices/list (`data`, `has_more`, `next_page`; voices with `accents[]` of `{accent, locale, is_native}`).
- Licence: Apache-2.0 (the SDK). https://docs.cartesia.ai/openapi.json sits behind a bot checkpoint (HTTP 429 to scripts), so the fragment is transcribed from the two sources above.
- Note: the two sources disagree on the voice locale field: the reference lists `accents[].locale`, the SDK types `locales[].locale` (and `accent` as a catalog id). The fragment keeps both and the adapter reads both.
