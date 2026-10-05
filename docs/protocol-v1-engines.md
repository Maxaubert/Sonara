# Sonara protocol v1: external engines

Part of [protocol v1](protocol-v1.md): speech engines the user adds at run time (capability
`engines`). Split out of `protocol-v1.md` to keep each file readable (#253); the rules of the
main document (transports, errors, versioning, additive changes) apply here too.

## Contents

- [Overview](#overview)
- [Never silent](#never-silent)
- [Muted: nothing is sent](#muted-nothing-is-sent)
- [Keys](#keys)
- [A key is bound to its address](#a-key-is-bound-to-its-address)
- [Profile](#profile)
- [Send to the engine](#send-to-the-engine)
- [Options](#options)
- [Kinds](#kinds)
  - [OpenAI-compatible servers (`openai-compatible`)](#openai-compatible-servers-openai-compatible)
  - [ElevenLabs (`elevenlabs`)](#elevenlabs-elevenlabs)
  - [Azure AI Speech (`azure`)](#azure-ai-speech-azure)
  - [Google Cloud Text-to-Speech (`google`)](#google-cloud-text-to-speech-google)
  - [Gemini (`gemini`)](#gemini-gemini)
  - [Cartesia (`cartesia`)](#cartesia-cartesia)
  - [Deepgram (`deepgram`)](#deepgram-deepgram)
  - [A program (`command`)](#a-program-command)
- [The profile view](#the-profile-view)
- [Messages](#messages)
- [The voice rule](#the-voice-rule)
- [Command line](#command-line)

## Overview

Protocol 1.2, capability `engines` (runtime 0.15.0, #224; the kinds `elevenlabs`, `azure` and
`google` since 0.16.0, #225; `cartesia`, `deepgram` and `command`, `engine_reload` and `E_FORBIDDEN`
(protocol 1.3) since 0.17.0, #226; `gemini` since 0.19.0, #235; design
`docs/plans/2026-10-04-external-engines-spec.md`). The user adds speech engines that are not part of
Sonara: a cloud API (OpenAI, ElevenLabs, Azure AI Speech, Google Cloud Text-to-Speech, Gemini,
Cartesia, Deepgram), a local server that speaks OpenAI's speech API, or a program of the user's own
on this PC (`command`). Each is a **profile** that becomes an engine next to `kokoro` and `onecore`
(licence class `external`), chosen with `set engine` like any other. Adding one sends nothing
anywhere: text goes to it only while it is the current engine (or in `engine_test`, a voice list).
The messages are core (no extension to enable) and work over TCP and HTTP (`POST /v1/engine_add`); a
runtime started with `--no-external-engines` answers each with `E_UNSUPPORTED`
(`this runtime does not allow external engines`).

## Never silent

**Never silent.** When the provider cannot speak a sentence (no key, a refused key, no credit, rate
limited, unreachable, a timeout, a server error, an unknown voice, bad settings, audio that is not
WAV or PCM), the runtime speaks that sentence with its built-in engine (Kokoro, which itself falls
back to OneCore) and, once per episode, prepends a short cue:
`<label> cannot be reached. Reading with the built-in voice.` (`has no key`, `refused the key`,
`is out of credit`, `is busy`, `has a server problem`, `does not know this voice`,
`settings do not work`). An episode ends with a success or a change of the profile or key.
`state.engine_status` of the engine shows the `reason` and the `fallback`: `waiting` while it will
try again by itself (the breaker after two transient failures in a row: 30 s, doubling to 300 s,
with no network wait meanwhile; `quota`: 10 minutes), `unavailable` while the user must change
something (`no_key`, `auth`, `bad_voice` for that voice, `bad_config`, `format`). A sentence goes to
the provider once (a 429 or 503 with a `Retry-After` of at most 1.5 s is tried once more); a longer
wait the provider asks for (`Retry-After`, Gemini's `retryDelay`) is kept from the first failure on:
nothing is sent before it ends (a rate limit up to 300 s; `quota` at least 10 minutes, up to 24
hours) (since 0.19.0, #235). A `quota` failure is never tried again at once. For an engine with a
slow round trip the reader synthesizes more sentences ahead (`options.prefetch`: 2 for a cloud
engine, 1 for a local one), and short repeated texts (spoken cues) are kept in memory.

## Muted: nothing is sent

**Muted: nothing is sent** (runtime 0.18.0, #227). While the reader is muted (`control mute`, the
mute hotkey without `agent`) or the agent's `mute_level` is 1 or 2 (also a saved level, from the
moment `agent` is enabled), no request of any kind goes to an external engine: every sentence (the
one playing, the ones synthesized ahead, a core `speak`, a "Session changed" announcement) and every
[spoken cue](protocol-v1.md#spoken-cues) is read with the built-in engine (Kokoro, else OneCore),
which is not a failure (no cue, no `reason`, no breaker), and voice lists are not fetched (`voices`
above). Muting cuts a request in flight at once, and that sentence is read with the built-in engine
too, so nothing more is sent or billed. The cues of a mute change (`"Muted."`, `"Super muted."`,
`"Unmuted."`) are always spoken with the built-in engine, the unmute's included. Two requests the
user makes on purpose still reach the engine while muted: `engine_test` (the Test button) and
[`preview`](protocol-v1.md#voice-previews); while the reader itself is muted (core `mute`) they are
still sent and billed but play silently, as every clip does. Mute changes apply one at a time, so
concurrent ones (a hotkey and another client) leave the engines held exactly while Sonara is muted,
and a sentence whose request was cut by a mute that lifted again at once is read with the built-in
engine, never dropped. Unmuted, the next sentence goes to the engine again.

## Keys

**Keys.** A key goes in only through `engine_add` (`secret`) or `engine_key` and is kept in Windows
Credential Manager (generic credential `sonara:<id>`, this user on this PC), or read from an
environment variable of the runtime's process (`key_ref` `env:NAME`). It is never in a file of the
home, a log line or a reply, and goes only in the provider's authentication header, over HTTPS or to
a loopback host. The `in` lines of the troubleshooting log drop the `secret` field entirely.

## A key is bound to its address

**A key is bound to its address** (runtime 0.15, #224). A stored key is kept with the origin
(`scheme://host:port`, the port always written) of the profile it was entered for: the `url`, else
the kind's default (`https://api.openai.com:443` for the `openai` preset; for later kinds the
provider's address, Azure's from `options.region`). A key is sent only to that origin: when the
profile now points elsewhere it is not sent, `key_present` is `false`, and the engine reports
`no_key` (`the key of 'x' was entered for <origin>, not <origin>: enter the key again`) and falls
back as usual. An `engine_add` replace that changes the origin deletes the stored key unless a
`secret` comes in the same request; changes that keep it (voice, model, label, options, another path
on the same host) keep the key. A key stored before 0.15 is bound to its profile's address in
`engines.json` at the first start of 0.15 (the file is local); one with no recorded address after
that (stored by an older runtime) is never sent: enter it again. An `env:` key is sent only to the
provider's default origin (the kind's or preset's address) or to the origin the user confirmed in
`engines.json` as `"key_origin": "https://host:443"` in that entry (restart the runtime after
editing): a `url` that arrives over the protocol, in a new profile or a replace, is never confirmed
by it (a `key_origin` in `engine_add` is ignored), and a replace to another origin drops the
confirmation. Entries of a format 1 file get `key_origin` set to their address when migrated. A
`command` key is bound to the program (`argv[0]`, case ignored) it was entered for, and is deleted
when `engines.json` names another program; an `env:` key may go to the program the file names.
Requests of an external engine follow no redirect (a `3xx` is an error and the engine falls back),
and a loopback engine never goes through a proxy. A key bound to a loopback address goes to whatever
program listens on that port, so do not store one for a local server you do not keep running; the
origin has no path, so a replace that changes only the path on the same host keeps the key.

## Profile

| field | rule |
|---|---|
| `id` | 1 to 32 of `a-z`, `0-9`, `-`, `_`, starting with a letter or digit; not `kokoro`, `onecore`, `fake`, nor starting with `sonara` |
| `kind` | `openai-compatible`, `elevenlabs`, `azure`, `google`, `gemini`, `cartesia`, `deepgram` or `command` in this runtime (`engine_list.kinds`); a profile of another kind (from a newer release) is kept and listed with `supported: false` |
| `label` | 1 to 40 characters; spoken in cues. Default: the preset's name (`openai-compatible`), else `ElevenLabs`, `Azure Speech`, `Google Text-to-Speech`, `Gemini`, `Cartesia`, `Deepgram` or `The speech program` |
| `url` | `openai-compatible`: the base URL with the API version, e.g. `https://api.openai.com/v1`, `http://127.0.0.1:8880/v1`; the cloud kinds: optional, below; `command`: none. HTTPS, or plain HTTP to a loopback host (`localhost`, `127.0.0.0/8`, `::1`), or with `options.allow_http` to a server on the network (then never with a key); no user, query or fragment |
| `model`, `voice` | up to 200 characters. **Sonara names no model or voice of its own** (since 0.19.0, #235): models and voices change at the provider, so they come from the provider's live lists (`engine_models`, `voices`) or are typed in. The model is required where the provider needs one in every request (`gemini`, `cartesia`, and the `openai`, `localai` and `speaches` presets) and optional where the provider picks its own when none is sent (`elevenlabs`, the other presets: then none is sent); `azure`, `google`, `deepgram` and `command` take none. Every kind but `command` needs a voice: [the voice rule](#the-voice-rule) says which one is used. A profile without a model or voice it needs is still valid (one stored by an older runtime keeps working): it reads with the built-in engine and its status is `bad_config`, "Choose a model for <label> in Sonara's settings (Engines)." or "Choose a voice for <label> in Sonara's settings.", until one is set |
| `key_ref` | `"none"`, `"credman"` or `"env:NAME"`, where `NAME` ends in `_API_KEY` or `_SPEECH_KEY`, is `SPEECH_KEY`, or starts with `SONARA_` (case-insensitive; any other variable is `E_BAD_REQUEST`, so a client cannot send the runtime's other secrets to a server it chose). Default: `none` for an `openai-compatible` loopback server and for `command`, `credman` otherwise (a cloud kind needs a key even behind a loopback URL) |
| `send_mode` | "Send to the engine" (since 0.19.0, #235): `"message"` or `"sentence"`, below. Optional: unset (or `null`) is the kind's default, `"sentence"` for `command` and an `openai-compatible` server at a loopback address, `"message"` for every other kind (a cloud kind also behind a local proxy, as for its key). Only a value the profile sets is stored; the view's `send_mode` is the mode in force and `explicit.send_mode` the stored one |
| `options` | per kind, below; an unknown option is `E_BAD_REQUEST` |

## Send to the engine

**Send to the engine** (`send_mode`, since 0.19.0, #235). `sentence`: every sentence is its own
request, sent as soon as the agent releases it (Kokoro and the Windows voices always read this way;
they are not profiles). `message`: what the agent releases for speech at once is one entry, one item
and **one request**: the reply at its end in read mode `done`, a batch in `queue`, a finished
paragraph in `immediate` (its blank line or the end of its block is the release point, so reading
starts after the first paragraph and each paragraph is one request), the prose held when a tool runs
(in `immediate` and `queue`; the paragraph and batch rules keep holding for the rest of the turn),
the prose held before a question, permission or plan (the decision itself stays its own short
request, read with priority), a summary, and core `speak` text. Sentences are joined with a space
and paragraphs with a blank line. The text is split only past the provider's input limit (or
`options.chunk_chars` when lower): at paragraph boundaries first, then between sentences, into as
few requests as possible; Previous and Next move by these parts, and Restart (Up) replays the whole
message. The audio plays as it arrives: Gemini's events, and a raw PCM answer read as it comes
(ElevenLabs' `/stream` endpoint with a `pcm_*` format, OpenAI-compatible `response_format: "pcm"`,
Cartesia); Azure, Google, Deepgram and a WAV answer are read whole before they play. No audio within
`first_audio_ms` (a streamed answer) or the request's time limit (a whole answer) is a failure of
that request: the built-in engine reads the whole text (the cue first, once per episode; one
`fallback` log line per request), playing as it is made. A request's time limit is `timeout_ms` plus
70 ms per character of its text (about the time the text takes to speak), so a long message is never
cut for its length; a streamed answer is cut only when it stalls (no audio for `first_audio_ms`) or
past that limit. An answer cut after audio came (a stall, a broken connection) keeps the audio
played, and the rest of the text is read with the built-in engine (the cue first, once per episode;
one `fallback` log line) from the start of the sentence the audio had reached: the place is
estimated from the seconds played at 12 characters a second, slower than most voices, so a sentence
may be read twice but none is dropped. A mute during a message reads the rest locally the same way
(no cue, no log line: a mute is no failure). The following requests of the message go on. A skip, a
flush, a new turn or a stop ends the item: its request is cancelled, a streamed answer's connection
is closed at the next data that arrives, and nothing more is sent for it. **Costs of a cancelled
request**: a whole answer already asked for is abandoned, and the provider may still finish and bill
it; a streamed answer whose first audio did not come within `first_audio_ms` is read by the built-in
engine, and its connection is dropped when the headers have not come by then, else at the next data,
so a provider that already started may still bill that request. The last four complete whole
messages are kept in memory (at most 15 million samples, about ten minutes of 24 kHz speech), so a
replay (Up after the item ended reads it again as a new item) sends no request; an answer cut off is
not kept. The `agent` and `read text` log lines show the joined text once.

## Options

At most 16 profiles. Options of every kind: `timeout_ms` (1000 to 120000; 15000 for a cloud engine,
60000 for a local server and for `gemini`, 30000 for a program; in send mode `message` a request
gets 70 ms more per character of its text), `prefetch` (1 to 4; 2 for a cloud engine, 1 for a local
one and for `gemini`), `allow_http` (boolean), `chunk_chars` (200 to 5000: the most characters one
request takes in send mode `message`, when lower than the provider's input limit; `gemini` 2000 by
default, about two minutes of speech), `first_audio_ms` (1000 to 60000, default 12000, never more
than `timeout_ms`: the longest a streamed answer may go without audio, before its first audio and
between two pieces). Options of `openai-compatible`: `preset` (below, default `generic`),
`response_format` (`wav`, the default, or `pcm`), `sample_rate` (8000 to 48000; raw PCM, and sent to
Speaches), `instructions` (never sent to `kokoro-fastapi` or the Chatterbox presets; sent as set to
any other server, such as LocalAI's expressive backends; a model that refuses it, a 400 naming
`instructions`, gets the sentence once more without it and none from then on), `extra` (an object
merged into the request body last, for a server's own fields), `voices_path` (the voice list path of
a `generic` server).

## Kinds

### OpenAI-compatible servers (`openai-compatible`)

| preset | model | voice list |
|---|---|---|
| `openai` | required | none (OpenAI has no voice list API: the voice is typed, from OpenAI's text-to-speech guide); default URL `https://api.openai.com/v1` |
| `kokoro-fastapi` | optional | `GET {url}/audio/voices`; sends `stream: false` |
| `localai` | required | `GET {url}/audio/voices?model=<model>` |
| `speaches` | required | `GET {url}/audio/voices`; sends `sample_rate` |
| `openedai-speech` | optional | none (typed) |
| `chatterbox-api` | optional | `GET {root}/voices` (`{root}`: the URL without `/v1`) |
| `chatterbox-server` | optional | `GET {root}/get_predefined_voices` (file names); always WAV |
| `generic` | optional | `GET {url}/audio/voices` (or `voices_path`); a failure is an empty list |

The model list of every preset is `GET {url}/models` (`{data: [{id}]}`): for `openai` the ids that
name `tts`; for a server, the models it marks as text-to-speech (`task`/`type`), else all it lists;
a server without the list gives an empty one (the model is typed). An unknown or retired model (a
404 `model_not_found`, a message naming the model) is `bad_config` whose message names it: "<label>
does not know the model '<model>' (404): ... Choose another model in Sonara's settings (Engines)."

A sentence is `POST {url}/audio/speech` with `{model?, input, voice, response_format, speed}`
(`model` only when the profile names one) (`speed` is the rate / 200, from 0.25 to 4.0) and
`Authorization: Bearer <key>` when there is a key. The answer is WAV (any rate, 16-bit or float) or
raw 16-bit mono PCM (rate from the `Content-Type` `rate=`, else `sample_rate`, else 24000); MP3,
Ogg, FLAC, JSON or HTML with a 200 status is `format`. With `response_format: "pcm"` (and for the
ElevenLabs, Azure, Google, Gemini, Cartesia and Deepgram kinds, which always ask for raw PCM) only
the `Content-Type` can name MP3, Ogg or FLAC: samples are never taken for a magic number.

### ElevenLabs (`elevenlabs`)

**ElevenLabs** (`kind: "elevenlabs"`). `url` default `https://api.elevenlabs.io`; `model` optional
(none sent: ElevenLabs uses its own default; the list is `GET {url}/v1/models`, those with
`can_do_text_to_speech`, an empty list when the key may not read it); `voice` (required) a voice id
from the voice list, a cloned voice's id included. Options: `output_format` (`pcm_24000`, the
default, `pcm_16000`, `pcm_22050`, `pcm_44100` on the Pro tier), `stability`, `similarity_boost`,
`style` (0 to 1), `language_code`, `enable_logging` (default `true`; `false` only matters on
enterprise accounts). A sentence is `POST {url}/v1/text-to-speech/{voice}?output_format=pcm_24000`
with `{text, model_id?, voice_settings?: {speed, ...}, language_code?}` (`voice_settings` is left
out when no `stability`, `similarity_boost` or `style` is set and the speed is 1.0, so the voice's
stored settings apply) and `xi-api-key: <key>`; the answer is raw 16-bit mono PCM at the named rate.
`speed` is the rate / 200 clamped to 0.7 to 1.2 (ElevenLabs' range: 140 to 240 words per minute map
exactly, faster rates stay at 1.2). Voice list: `GET {url}/v2/voices`, all pages.

### Azure AI Speech (`azure`)

**Azure AI Speech** (`kind: "azure"`). `options.region` (e.g. `westeurope`) gives
`https://{region}.tts.speech.microsoft.com`; or `url` names the endpoint (a custom domain). `voice`
(required) a voice's short name from the voice list (the name starts with its locale, for example
`en-US-...`; names change at Microsoft, so Sonara names none). Options: `region`, `output_format`
(`raw-24khz-16bit-mono-pcm`, the default, or `raw-8khz-`, `raw-16khz-`, `raw-22050hz-`,
`raw-44100hz-`, `raw-48khz-16bit-mono-pcm`), `lang` (the SSML `xml:lang`; default the voice name's
locale). A sentence is `POST {base}/cognitiveservices/v1` with SSML (the text XML-escaped,
`<prosody rate>` the rate / 200 from 0.5 to 2.0), `Ocp-Apim-Subscription-Key: <key>`,
`X-Microsoft-OutputFormat` and `User-Agent: Sonara/<version>`. A 401 is often a key of another
region. A 400 for a voice that is not in the fetched voice list is `bad_voice`. Voice list:
`GET {base}/cognitiveservices/voices/list`, or `GET {base}/tts/cognitiveservices/voices/list` when
the url is a resource host (`<resource>.cognitiveservices.azure.com`; not verified live).

### Google Cloud Text-to-Speech (`google`)

**Google Cloud Text-to-Speech** (`kind: "google"`). `url` default
`https://texttospeech.googleapis.com`; `voice` (required) a voice name from the voice list (it
starts with its locale). Options: `language_code` (default the voice name's locale, else `en-US`),
`sample_rate` (8000 to 48000, default 24000), `user_project` (sent as `x-goog-user-project`),
`model_name` (`voice.modelName`, for Gemini TTS voices; not verified live). A sentence is
`POST {url}/v1/text:synthesize` with
`{input: {text}, voice: {languageCode, name}, audioConfig: {audioEncoding: "PCM", sampleRateHertz, speakingRate}}`
(`speakingRate` the rate / 200 from 0.25 to 2.0) and `X-goog-api-key: <key>` (an API key of a
project with the Text-to-Speech API on; never in the URL); the answer's `audioContent` is base64
PCM. A sentence over 5000 UTF-8 bytes is sent in parts. Voice list: `GET {url}/v1/voices`.

### Gemini (`gemini`)

**Gemini** (`kind: "gemini"`, since 0.19.0, #235). The Gemini API's speech models (a key from Google
AI Studio, aistudio.google.com, Get API key). `url` default
`https://generativelanguage.googleapis.com`; `model` (required: it is in the URL) a Gemini speech
model id, from the list `GET {url}/v1beta/models` (the models whose id names `tts` and that take
`generateContent`; paged, `pageToken`); `voice` (required) from the list `GET {url}/v1beta/voices`
(the caller's stored voices, then Google's prebuilt catalog; paged, `page_token`), or a stored
voice's id (`voice_...`). Both lists are fetched with the key. Options: `language_code` (a BCP-47
locale, sent as `speechConfig.languageCode`; default none, Gemini tells from the text), `style` (at
most 500 characters, a direction such as `calm and warm`); `chunk_chars` and `first_audio_ms` as for
every kind (below). **Streamed** (since 0.19.0, #235): a sentence is
`POST {url}/v1beta/models/{model}:streamGenerateContent?alt=sse`; each server-sent event is a
`GenerateContentResponse` with the next audio, which plays as it comes (the reader starts a chunk at
its first audio). When no audio has come after `first_audio_ms` (never more than `timeout_ms`), the
sentence is a `timeout` and reads with the built-in engine, with the cue once per episode (a slow
free tier no longer holds a sentence for a minute); an answer that stalls after audio came (no audio
for `first_audio_ms`) or breaks off keeps that audio, and in send mode `message` the rest is read
with the built-in engine from the sentence reached (above); in `sentence` the sentence ends with
what came (the log says why). A model that refuses the stream (a 400 or 404 naming
`streamGenerateContent`) is asked with `:generateContent` (the whole answer at once) from then on
until the runtime restarts. The body is
`{contents: [{role: "user", parts: [{text, speechMetadata?: {style}}]}], generationConfig: {responseModalities: ["AUDIO"], speechConfig: {voiceConfig: {prebuiltVoiceConfig: {voiceName}}, languageCode?}, responseFormat: {audio: {mimeType: "AUDIO_L16", sampleRate: 24000}}}}`
(a stored voice id, `voice_...` or `voicekey_...`, goes as `voiceConfig: {voice}`) and
`x-goog-api-key: <key>` (never in the URL); the answer's
`candidates[0].content.parts[].inlineData.data` is base64 PCM (a WAV is read too). Gemini has **no
speed**: the rate goes in `speechMetadata.style` after the profile's `style`, as `speaking slowly`
(at most 160 words per minute), nothing (161 to 280, around the default rate of 250),
`speaking quickly` (281 to 350) or `speaking very quickly`; it is a direction, not a measure, and
never read aloud. A model that refuses `responseFormat` or `speechMetadata` (a 400 naming the field)
gets the sentence once more without it, and the engine stops sending it until the runtime restarts;
a model that refuses both costs two extra requests once. **Requests**: the free tier limits requests
per minute and per day, so Gemini sends whole messages by default (send mode `message`, above): a
reply of 18 sentences read when done is one request (it was 18), streamed so it plays from its first
audio; a message over `chunk_chars` (default 2000) is sent in as few parts as fit. The 0.19
pre-release options `quick_start` (dropped when read) and `chunk_chars: 0` (read as
`send_mode: "sentence"`) are folded into `send_mode`. Errors: 401, 403, and a 400 `API_KEY_INVALID`
are `auth`; a 400 naming an unknown voice is `bad_voice`; a 404 or a 400 about the model (unknown,
retired, not supported) is `bad_config` naming the model ("Gemini does not know the model '<model>'
... Choose another model"); other 400s (`FAILED_PRECONDITION`, a region the API does not serve) are
`bad_config`; 429 is `quota` when Google names a daily or spend limit, else `rate_limited`, both
waiting for the `retryDelay`; a `...PerDay...` quota id waits at least until the next 08:00 UTC
(midnight Pacific standard time, when Google resets requests per day; an hour after the reset in
summer) and at least an hour; 5xx is `server`; a 200 with no audio (a blocked text) is `server` for
that sentence only. On the free tier Google may use the text to improve its products; with billing
on, it does not.

### Cartesia (`cartesia`)

**Cartesia** (`kind: "cartesia"`). `url` default `https://api.cartesia.ai`; `model` (required) a
model id from Cartesia's documentation (Cartesia has no model list Sonara uses; a `model_not_found`
names it); `voice` (required) a voice id from the voice list (a uuid; a cloned voice's id included).
Options: `api_version` (the `Cartesia-Version` date, default `2026-08-14`; a version Cartesia
retires answers 400, and the message names this option), `language` (default `en`), `sample_rate`
(8000, 16000, 22050, 24000 (the default), 44100 or 48000). A sentence is `POST {url}/tts/bytes` with
`{model_id, transcript, voice: {id}, output_format: {container: "raw", encoding: "pcm_s16le", sample_rate}, language, generation_config?: {speed}}`
(`generation_config` left out at speed 1.0; `speed` the rate / 200 from 0.6 to 1.5),
`Authorization: Bearer <key>` and `Cartesia-Version`; the answer is raw 16-bit mono PCM. Voice list:
`GET {url}/voices?limit=100`, all pages (`starting_after`).

### Deepgram (`deepgram`)

**Deepgram** (`kind: "deepgram"`). `url` default `https://api.deepgram.com` (EU:
`https://api.eu.deepgram.com`); `voice` (required) is the speech model, one of the `tts` models of
the voice list; no `model`. Option: `sample_rate` (8000, 16000, 24000 (the default), 32000 or
48000). A sentence is
`POST {url}/v1/speak?model={voice}&encoding=linear16&container=none&sample_rate=24000[&speed=]` with
`{text}` and `Authorization: Token <key>`; the answer is raw 16-bit mono PCM. `speed` (the rate /
200 from 0.7 to 1.5) is left out at 1.0; Deepgram's support for it is not confirmed, so when
Deepgram refuses it (a 400 whose error body names `speed`, for a sentence that sent it) the sentence
is sent once more without it and the engine stops sending it until the runtime restarts. Voice list:
the `tts` models of `GET {url}/v1/models`.

### A program (`command`)

**A program** (`kind: "command"`). A program of the user's own on this PC speaks, for example a
Piper install; nothing is bundled, and the text stays on the PC. **It is configured only locally,
never over the protocol**: `engine_add` with `kind: "command"`, or one that would replace an
existing `command` profile (with any kind), is `E_FORBIDDEN` over TCP and HTTP, from any client or
SDK, before any other check (so a reply never says whether a path exists); the message is
`a command engine runs a program on this PC, so it is never added or changed over the protocol: add it with `sonara
engines add <id> --kind command`, or in engines.json`. The user adds one by editing `engines.json`
in the home, or with `sonara engines add <id> --kind command --option 'argv=[...]'`, which writes
`engines.json` as the user and then sends `engine_reload` (which takes no profile); listing,
`engine_test`, selecting it with `set engine`, `engine_key` and `engine_remove` work over the
protocol as for any profile. The reason: the token is shared with the settings page in the browser
and with every client, so a protocol message that names a program to run would turn any exposure of
the token into running code. `options.argv` (required) is the program's full path (an `.exe` that
exists: checked by `sonara engines add` and again before every start; a `.bat` or `.cmd` is refused,
as the command shell would read its arguments as commands) and its arguments, at most 64. It is
started directly, never through a shell (`cmd /c`) or a command line built from text: each `argv`
entry is passed as one argument (quoted for Windows by the runtime), so characters such as `&`, `|`,
`>`, `^`, `%` or `"` in an argument reach the program as they are. **The text read is never an
argument**: `{text}` in `argv` is refused (the entry is listed with `error`). Placeholders inside
any argument, filled in one pass (a filled-in value is not scanned again): `{voice}`, `{rate}`
(words per minute), `{speed}` (the rate / 200, two decimals), `{out}` (a temporary `.wav` path,
deleted afterwards), `{in}` (a temporary UTF-8 `.txt` file holding the text, deleted afterwards). A
voice that goes into `{voice}` must be one of `options.voices` when that list is set, and may not
start with `-` (it could read as an option) or hold control characters; without that list it must
also be a plain name (letters, digits, `_`, `-` and `.`, no `..`), never a path such as
`\host\share\m.onnx` or `C:\x.onnx`, so a client cannot change what the program loads (a model path
belongs in `options.voices`, which only the user writes); otherwise the program is not started
(`bad_config`). Options: `input` (`stdin`, the default: the text in UTF-8, then closed; or `file`:
the text in the file at `{in}`, which must be in `argv`), `output` (`stdout-wav`, the default;
`stdout-pcm`: raw 16-bit mono at `sample_rate`, which it then needs; `file`: a WAV at `{out}`, which
must be in `argv`), `sample_rate` (8000 to 48000), `voices` (the names offered as its voice list). A
key, when `key_ref` names one, is passed in the environment variable `SONARA_ENGINE_KEY`, never as
an argument. Outcomes: a program that cannot start (or no longer exists) is `bad_config`, an exit
code other than 0 is `server` (with the last line of its error output, with the key cut out and
masked), no output or output that is not WAV/PCM is `format`, and over `timeout_ms` (default 30 s)
or on a cancel the process is killed (`timeout`), with every process it started (a Job Object); a
process it leaves running when it exits is ended too, so a program must not hand its work to a child
that outlives it. A stored profile whose program went away stays and reads with the fallback until
it is back.

## The profile view

**The profile view** (in replies; never a key): the profile with the address in force (the preset's
or kind's default `url` filled in; the `model` and `voice` only as the profile sets them, never a
default), plus `explicit` (since 0.18.0, #227: the `url`, `model`, `voice` and, since 0.19.0,
`send_mode` the profile sets itself; an edit form starts from these so a default, such as an Azure
region's endpoint, stays a default), `takes_model`, `model_required` and `model_list` (since 0.19.0,
#235: whether the kind has a model, needs one, and whether the provider lists its models), `missing`
(since 0.19.0: what the user must still pick, `"model"` and/or `"voice"`; the voice is not missing
for the current engine while the voice setting names one), `key_present` (a key resolves now, also
for `env:`), `sends_text_to` (the URL's host; for `command` `program <file name>`), `local` (a
loopback host or a program), `send_mode` (since 0.19.0, #235: the send mode in force, the profile's
or the kind's default), `license_class: "external"`, `supported`, `current` (it is the reader's
engine) and `status` (its `engine_status` without `engine`; `null` when not usable). An entry that
cannot be used has `error`.

```json
{"id": "openai", "kind": "openai-compatible", "label": "OpenAI", "url": "https://api.openai.com/v1", "model": "<model>", "voice": "<voice>", "key_ref": "credman", "options": {"preset": "openai"}, "explicit": {"model": "<model>", "voice": "<voice>"}, "takes_model": true, "model_required": true, "model_list": true, "missing": [], "key_present": true, "sends_text_to": "api.openai.com", "local": false, "send_mode": "message", "license_class": "external", "supported": true, "current": false, "status": {"ready": true, "status": "ready"}}
```

## Messages

A request's `id` is its correlation id (echoed in the reply), so these messages name a profile with
`engine`.

| message | fields | reply |
|---|---|---|
| `engine_list` | none | `{engines: [view...], builtin: ["kokoro", "onecore"], kinds: ["openai-compatible", "elevenlabs", "azure", "google", "gemini", "cartesia", "deepgram", "command"], presets: [...]}` |
| `engine_add` | `engine` (the profile), `secret?` (stored as its key; sets `key_ref` `credman` when absent, `E_BAD_REQUEST` with `env:` or `none`), `replace?` (default `false`) | `{engine: view}`. Validates, saves `engines.json`, stores the key, registers the engine; it does not select it. Kind `command`, or an id that is a `command` profile, is `E_FORBIDDEN` (protocol 1.3; above). An existing id without `replace: true` is `E_BAD_REQUEST`; a replace keeps the stored key unless `secret` is given or the origin changes (then the key is deleted, see Keys), forgets the engine's failures and cached audio, and applies to the next sentence when it is the current engine |
| `engine_remove` | `engine`, `forget_key?` (default `true`) | `{removed: "<id>", engine: "<engine now in force>"}`. The current engine is first switched to the default choice (Kokoro when installed, else OneCore; saved like any `set`); then the engine is unregistered, removed from `engines.json` and its stored key deleted |
| `engine_key` | `engine`, `secret` (a string, or `null` to delete) | `{engine, key_present}`; the key is bound to the profile's origin now; clears a `no_key` or `auth` block. `E_BAD_REQUEST` for a profile whose `key_ref` is `env:` or `none` |
| `engine_reload` | none (protocol 1.3; any other field is ignored: it never takes a profile) | `engine_list`'s fields plus `problems` (the lines also logged). Reads `engines.json` again after the user or `sonara engines add --kind command` changed it: an unchanged profile keeps its engine, a changed one is replaced (applies to the next sentence when current), one that is gone or now unusable is unregistered, and a current engine that went away is switched to the default choice (saved like any `set`). A stored key whose address (or, for a `command`, program) is no longer its entry's is deleted (see Keys); other keys are untouched. A file that is not JSON is `E_BAD_REQUEST` and changes nothing. `engine_add` and `engine_remove` first read `engines.json` again when it changed on disk since the runtime last read or saved it, so a save never writes an older list over the user's edit (a file that is not JSON fails them the same way and is not overwritten) |
| `engine_test` | `engine`, `text?` (default "Hello. This is how Sonara sounds with this voice.", at most 300 characters), `voice?`, `play?` (default `true`) | `{engine, voice, ms, sample_rate, duration_ms}`. One synthesis at the current rate with no fallback and no cache, with the voice of [the voice rule](#the-voice-rule), played as a clip over whatever is read (as `preview`) when `play`; it is sent even while Sonara is muted (the user asked for it). A success clears the engine's blocks and breaker; a failure is `E_ENGINE` with `reason` |
| `engine_models` | `engine` and `refresh?`, or `profile` (an unsaved profile, as for `voices`) and `secret?` (protocol 1.5, #235) | `{models: [{id, name}], list, takes_model, required, error?}`: the provider's models now (`list`: it has a model list API; else the model is typed, and `models` holds only the profile's own). A saved engine's list is cached as its voices are (fetched again after 10 minutes or with `refresh`; a failure is not retried for a minute); a failed fetch keeps the known models and adds `error: {reason, message}`. Nothing is fetched while Sonara is muted (`error.reason` `muted`). A draft is built for this request only, with `secret` as its only key, as for `voices`; never logged |

```json
> {"type": "engine_add", "engine": {"id": "openai", "kind": "openai-compatible", "options": {"preset": "openai"}}, "secret": "sk-...", "id": 1}
< {"id": 1, "ok": true, "engine": {"id": "openai", "kind": "openai-compatible", "url": "https://api.openai.com/v1", "key_ref": "credman", "key_present": true, "missing": ["model", "voice"], "sends_text_to": "api.openai.com", "current": false, ...}}
> {"type": "engine_models", "engine": "openai", "id": 4}
< {"id": 4, "ok": true, "models": [{"id": "<model>", "name": "<model>"}], "list": true, "takes_model": true, "required": true}
> {"type": "engine_add", "engine": {"id": "openai", "kind": "openai-compatible", "model": "<model>", "voice": "<voice>", "options": {"preset": "openai"}}, "replace": true, "id": 5}
> {"type": "set", "key": "engine", "value": "openai", "id": 2}
< {"id": 2, "ok": true, "key": "engine", "value": "openai"}
> {"type": "engine_test", "engine": "openai", "play": false, "id": 3}
< {"id": 3, "ok": false, "error": {"code": "E_ENGINE", "message": "OpenAI refused the key (401): Incorrect API key provided", "reason": "auth"}}
```

| error | code |
|---|---|
| a profile or message field breaks a rule | `E_BAD_REQUEST` |
| an unknown profile id | `E_NOT_FOUND` |
| `engine_add` of a `command` engine, or replacing one | `E_FORBIDDEN` |
| the runtime refuses external engines; a kind this runtime lacks | `E_UNSUPPORTED` |
| Credential Manager failed; `engine_test` failed | `E_ENGINE` |

## The voice rule

One rule decides the voice of an external engine everywhere (#235): the voice a request names
(`engine_test` `voice`, `preview` `voice`), else the user's voice setting (`set voice`) while that
engine is the current one, else the profile's `voice`. Reading, the
[preview](protocol-v1.md#voice-previews) and `engine_test` (the settings page's Test button,
`sonara engines test`) all follow it, so the voice picked in Sonara is the one heard. With none of
them the sentence reads with the built-in engine and the status says "choose a voice". Model ids and
voice names in this document are placeholders such as `<model>` and `<voice>`: the provider's own
lists name the current ones.

## Command line

The command line: `sonara engines list|add|key|use|test|models|voices|remove`
(`sonara engines help`; `models <id>` and `voices <id>` print the provider's lists, since 0.19.0); a
key is read from stdin or a prompt without echo, never from an argument.
`sonara engines add <id> --kind command` writes `engines.json` itself and sends `engine_reload`; an
entry the runtime then lists with `error` is taken out of the file again. The SDKs: `client.engines`
(`@sonara/client` `EnginesApi`, `sonara-client` `Engines`, with `reload()` and, since 0.19.0,
`models()`); their `add` of a `command` engine is `E_FORBIDDEN` like any client's.
