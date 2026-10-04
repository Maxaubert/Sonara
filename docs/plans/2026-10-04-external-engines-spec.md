# External TTS engines: design spec and implementation plan

Status: written 2026-10-04 for issues #224 to #227, from the user-approved direction and the provider research of the same day (facts and sources in section 13). One document holds the spec (sections 1 to 12) and the implementation plan (section 14), for a single approval.

Four PRs, stacked, each a feature with its own version bump:

| PR | Issue | Branch | Version | Content |
|---|---|---|---|---|
| PR1 | #224 | `feat/224-http-engine` | 0.15.0 | Core: profiles, `engines.json`, keys, Registry changes, `External` engine with fallback, cue, breaker, prefetch depth, cue cache, the `openai-compatible` kind (OpenAI and local servers), protocol `engine_*` messages, CLI, SDK helpers, conformance |
| PR2 | #225 | `feat/225-cloud-adapters` | 0.16.0 | Kinds `elevenlabs`, `azure`, `google` |
| PR3 | #226 | `feat/226-more-engines` | 0.17.0 | Kinds `cartesia`, `deepgram`, `command` |
| PR4 | #227 | `feat/227-engines-settings` | 0.18.0 | Settings page Engines section; README engines section |
| Gemini | #235 | `feat/235-gemini-engine` | 0.19.0 | Kind `gemini` (added after PR4, user request of 2026-10-05; section 14 "Gemini") |

PR2 branches from PR1, PR3 from PR2, PR4 from PR3. Each PR is reviewed and merged on its own; nothing is merged without the user's "merge?" answer.

---

## 1. Goals and non-goals

**Goals**

- G1. Any outside speech engine is a **profile** the user adds at run time: a cloud API (OpenAI, ElevenLabs, Azure, Google, Cartesia, Deepgram), a local OpenAI-compatible server (Kokoro-FastAPI, LocalAI, Speaches, openedai-speech, Chatterbox wrappers) or a local program (`command`). A profile is an `Engine` in `sonara-engine`'s `Registry`, next to `kokoro` and `onecore`, and is chosen with the existing `set engine`.
- G2. One HTTP core, one small adapter per provider shape. PCM or WAV only: no MP3, Opus, AAC or FLAC decoder is added.
- G3. Keys live in Windows Credential Manager (target `sonara:<profile id>`) or in an environment variable the profile names. Never in a file in the home, never in a log line, never in a reply.
- G4. Never silent: any failure of a profile (network, auth, quota, rate limit, bad voice, bad answer) speaks that chunk with the built-in engine (Kokoro, which itself falls back to OneCore), says once why, logs the reason and shows it in `engine_status`.
- G5. Cloud engines are strictly opt-in: nothing sends text off the PC until the user adds a profile **and** selects it.
- G6. Latency close to Kokoro's for reading whole replies: synthesis runs ahead of playback (prefetch depth per engine), short repeated texts (cues) are cached.
- G7. Hosts that bundle Sonara can refuse external engines (licence class `External`).

**Non-goals**

- No WebSocket or gRPC streaming APIs (ElevenLabs stream-input, Google streamingSynthesize, Cartesia WebSocket). HTTP request per chunk only.
- No playback of a chunk before its synthesis finished (the synth thread collects a whole chunk today; see 7.4 for why that is enough and what a later change would do).
- No Amazon Polly (needs SigV4 and caps PCM at 16 kHz; listed as possible later work) and no PlayHT (the service shut down on 2025-12-31).
- No OAuth flows (Google service accounts, Azure Entra). Keys only.
- No server-side voice cloning or voice creation calls. A cloned voice is used by pasting its id.
- No change to the Python package `src/sonara`.

## 2. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | Profiles persist in a **new `engines.json`** in the home, not in `config.json` | `config.json` holds flat user-set settings with a schema and default-merging; profiles are a collection with add, replace and remove. A separate file keeps the settings migration and `config.json.bad` logic untouched, and `sonara uninstall --keep settings` keeps it with the other settings |
| D2 | Protocol: **new core messages** `engine_list`, `engine_add`, `engine_remove`, `engine_key`, `engine_test`, capability `engines`, protocol **1.2** | `engine` and `voices` are core, so their management is too; a capability (not an extension) lets a host that refuses external engines leave it out. All additive: no existing message changes meaning |
| D3 | `EngineId` stays `Copy` and `&'static str`; profile ids are **interned** (`EngineId::intern`) | Avoids touching every user of `EngineId`. At most 16 profiles with ids of at most 32 bytes, each distinct id leaked once per process: bounded |
| D4 | `Registry` gets **interior mutability** (`register(&self)`, `unregister(&self)`) | The reader holds `Arc<Registry>`; profiles come and go while it runs |
| D5 | New `LicenseClass::External`. `Registry::default()` stays `[Permissive, Os]` (library hosts opt in); `sonarad` builds its registries with `[Permissive, Os, External]` unless `--no-external-engines` | "Allowed in sonarad, refusable by hosts" |
| D6 | The `External` engine **owns its fallback** (like Kokoro owns OneCore): `sonarad` passes Kokoro (whose own fallback is OneCore) | Same pattern as today; the reader needs no fallback logic; `engine_status.fallback` already exists |
| D7 | The one-time cue is **prepended to the chunk's audio** by the engine, synthesized with the fallback engine | Works with or without the `system` extension, in order with the text, never touches the queue |
| D8 | `openai-compatible` asks for **WAV by default** and sniffs the body (RIFF, else raw s16le) | WAV is the one format every researched server returns correctly (LocalAI turns `pcm` into WAV, Chatterbox-TTS-Server rejects `pcm`, Speaches and openedai vary the PCM rate). `wav::decode` already handles float WAV, LIST chunks and placeholder sizes |
| D9 | Each HTTP request runs on a **short-lived worker thread**; `synthesize` waits on a channel that `cancel` can end at once | `ureq` 3 is blocking with no abort; the reader requires a prompt end after `cancel` |
| D10 | Prefetch depth comes from the engine (`Engine::lookahead`, default 1; `External` 2 for cloud, 1 for loopback) and the pure reader state machine gets `set_lookahead` | The reader already synthesizes one chunk ahead (`sonara-core` `prefetch`); one more covers a cloud round trip of up to one sentence's playing time |
| D11 | Speed: Sonara wpm / 200 (the Kokoro baseline) mapped and clamped per provider | One rule everywhere; documented clamps per provider |
| D12 | Testing aids mirror `--system fake`: `sonarad --keys fake` keeps keys in `<home>\fake-keys.json` | Conformance and e2e never touch the real Credential Manager |

## 3. Architecture

```
protocol (sonarad/src/engines_ext.rs)            CLI (sonara engines ...)   SDKs (engines helpers)
   engine_list / engine_add / engine_remove / engine_key / engine_test, voices {engine, refresh}
        |
        v
sonarad::engines (profiles.rs: engines.json, ids, validation; manager: register into the reader's
        |          and the previews' Registry, keystore choice, notice -> sonarad.log)
        v
sonara_engine::external::External  (impl Engine; one per profile)
   |-- Backend: Http(adapter) | Command          (how one request is made)
   |-- keys::KeyResolver (credman | env:NAME | none) over a KeyStore
   |-- health::Health (breaker, blocked reason, cue-once set)
   |-- cache::CueCache (LRU of short texts)
   |-- fallback: Arc<dyn Engine> (Kokoro -> OneCore)
        v
sonara_reader worker -> synth thread (one job at a time, lookahead from the engine)
```

The reader, channels, agent and system layers do not learn anything about profiles: they see engines.

## 4. Module layout

### 4.1 `crates/sonara-engine` (feature `external`, on in `sonarad`)

```
src/
  lib.rs                 + pub mod external (cfg feature), Engine trait additions (4.3)
  types.rs               + LicenseClass::External, EngineId::intern
  registry.rs            interior mutability, unregister
  http.rs        (new)   shared ureq Agent builder (moved out of kokoro/download.rs), Response helpers
  external/
    mod.rs               External engine (impl Engine), ExternalConfig, synth path, fallback, cue
    profile.rs           Profile, Kind, Preset, validation (no serde derive: serde_json::Value in/out)
    keys.rs              Secret, KeyRef, KeyStore trait, CredentialStore (Windows), MemoryStore, FileStore (fake)
    error.rs             ExtError, Reason (+ is_transient, cue text, wire code)
    health.rs            Health: breaker, blocked state, cue-once set, injectable clock
    cache.rs             CueCache (LRU)
    audio.rs             body -> PcmChunk: sniff RIFF/raw/mp3/json, odd-byte carry, rate from Content-Type
    rate.rs              wpm -> provider speed / SSML prosody
    split.rs             split a chunk longer than a provider's input limit (chars or UTF-8 bytes)
    worker.rs            run one blocking request on a thread, cancellable wait
    adapter.rs           Adapter trait, HttpRequest/HttpReply, shared error-body parsing
    openai.rs            PR1: kind openai-compatible, presets
    elevenlabs.rs        PR2
    azure.rs             PR2 (+ SSML builder and XML escaping)
    google.rs            PR2 (+ base64 decode)
    cartesia.rs          PR3
    deepgram.rs          PR3
    command.rs           PR3: Backend::Command (not an Adapter)
tests/
  common/mod.rs          + scripted POST server: per-route status, headers, body, delay, captured requests
  external_core.rs       fallback, breaker, cue, cancel, cache, split (with a scripted adapter)
  external_openai.rs     PR1, against the scripted server
  external_cloud.rs      PR2
  external_more.rs       PR3
  external_live.rs       #[ignore] live tests, keys from env vars
  credman_live.rs        #[ignore] real Credential Manager round trip
```

`Cargo.toml` of `sonara-engine`:

```toml
[features]
external = ["dep:ureq", "dep:serde_json", "dep:windows"]   # PR2 adds "dep:base64"

[target.'cfg(windows)'.dependencies]
windows = { workspace = true, optional = true, features = ["Win32_Foundation", "Win32_Security_Credentials"] }
```

Reuse the existing `ureq` entry (rustls, platform-verifier, win-system-proxy). PR2 adds `base64` at the version already in `Cargo.lock` (0.23.x today, MIT OR Apache-2.0) for Google; PR3 adds Windows process features (`Win32_System_Threading` only if `std::process` with `CREATE_NO_WINDOW` via `std::os::windows::process::CommandExt` is not enough: it is, so none is expected). After any dependency change: `python packaging/notices/gen_notices.py`.

### 4.2 `crates/sonarad`

```
src/
  engines.rs     (new)  Engines manager: load/save engines.json, build External per profile,
                        register/unregister in the reader's and the previews' Registry, keystore,
                        notice sink -> support log, profile views for replies
  engines_ext.rs (new)  protocol handlers engine_list/add/remove/key/test, voices refresh
  protocol.rs           dispatch + CAPABILITIES "engines" (when allowed) + PROTOCOL_MINOR 2
  args.rs               --no-external-engines, --keys windows|fake
  main.rs               registries with External allowed, load profiles before the reader starts
  trace_log.rs          engine_key/engine_add: `secret` always dropped from `in` lines (test)
```

### 4.3 Other crates

- `sonara-core/src/reader/mod.rs`: `lookahead: usize` (default 1, range 1..=4), `set_lookahead(n) -> Vec<Effect>` (requests the newly allowed chunks when playing), `prefetch` requests up to `lookahead` chunks after the playing one, across item boundaries (the next queued items' first chunks count).
- `sonara-reader`: worker calls `reader.set_lookahead(engine.lookahead())` at init and on every engine switch; `settings::resolve_voice` accepts any non-empty id when `engine.accepts_unlisted_voices()`; `ReaderHandle::refresh_voices(engine_id)`.
- `sonara-log/src/secrets.rs`: add token prefixes `sk_car_` (Cartesia), `sk-proj-` is covered by `sk-`; add secret field names `secret`, `xi-api-key`, `ocp-apim-subscription-key`, `x-goog-api-key` (the `key` fragments are already covered by `api-key`/`api_key` for the first, the others are added explicitly).
- `sonara-cli`: `sonara engines list|add|remove|key|use|test` (4.4) and uninstall removing credentials.
- `clients/ts`: `src/engines.ts` (`EnginesApi`), exported from `index.ts`, `client.engines`.
- `clients/python`: `src/sonara_client/engines.py` (`EnginesApi`), `client.engines`.

### 4.4 Engine trait additions (`sonara-engine/src/lib.rs`)

All have defaults, so `onecore`, `kokoro` and `fake` do not change:

```rust
pub trait Engine: Send + Sync {
    // ... existing methods ...

    /// How many chunks the reader should synthesize ahead of the playing one (1..=4).
    fn lookahead(&self) -> usize { 1 }

    /// True when `synthesize` accepts voice ids that `voices()` does not list
    /// (cloud voice ids, cloned voices, file names of a local server).
    fn accepts_unlisted_voices(&self) -> bool { false }

    /// Fetch the voice list from its source (network for External; may block,
    /// bounded by a timeout). `voices()` stays cheap and returns the last list.
    fn refresh_voices(&self) -> Result<Vec<Voice>> { Ok(self.voices()) }
}
```

`EngineStatus` gains `reason: Option<Reason>` where `Reason` is re-exported from `external::error` but defined in `types.rs` so it exists without the feature:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason { NoKey, Auth, Quota, RateLimited, Network, Timeout, Server, BadVoice, BadConfig, Format }
impl Reason { pub fn as_str(&self) -> &'static str { /* "no_key", "auth", "quota", "rate_limited",
    "network", "timeout", "server", "bad_voice", "bad_config", "format" */ } }
```

`LicenseClass::External` serializes as `"external"` (`wire::voice_json`).

`EngineId::intern(id: &str) -> EngineId`: a process-wide `Mutex<HashSet<&'static str>>`; returns the existing `&'static str` or leaks a new `Box<str>`. Callers validate ids first (5.2), so the set is bounded.

`Registry`:

```rust
pub struct Registry { allowed: Vec<LicenseClass>, engines: RwLock<Vec<Arc<dyn Engine>>> }
impl Registry {
    pub fn register(&self, engine: Arc<dyn Engine>) -> Result<()>;      // was &mut self
    pub fn unregister(&self, id: &str) -> Result<Arc<dyn Engine>>;      // UnknownEngine if absent
    pub fn replace(&self, engine: Arc<dyn Engine>) -> Result<()>;        // same id, atomically
    // get, ids, voices, allows unchanged in meaning
}
```

`Error` gains `External { reason: Reason, message: String }` (Display: the message, already masked).

## 5. Profiles

### 5.1 Type

```rust
pub struct Profile {
    pub id: String,                 // engine id, see 5.2
    pub kind: Kind,
    pub label: Option<String>,      // spoken in cues and shown in UIs; default per kind/preset
    pub url: Option<String>,        // base URL (kind-specific meaning, 5.4)
    pub model: Option<String>,
    pub voice: Option<String>,      // default voice when the reader's voice is null
    pub key_ref: KeyRef,
    pub options: serde_json::Map<String, Value>,   // kind-specific, validated per kind
}
pub enum Kind { OpenAiCompatible, ElevenLabs, Azure, Google, Cartesia, Deepgram, Command }
// wire names: "openai-compatible", "elevenlabs", "azure", "google", "cartesia", "deepgram", "command"
pub enum KeyRef { None, CredMan, Env(String) }
// wire: null or "none" | "credman" | "env:NAME"
```

A kind this build does not implement yet (a file written by a newer release, or `elevenlabs` before PR2) is kept in `engines.json` untouched, listed by `engine_list` with `"supported": false`, and not registered.

### 5.2 Validation (`profile.rs`, `Profile::validate`)

| Field | Rule | Error (`E_BAD_REQUEST` message) |
|---|---|---|
| `id` | `^[a-z0-9][a-z0-9_-]{0,31}$`; not `kokoro`, `onecore`, `fake`, not starting with `sonara` | `invalid engine id 'x': use 1 to 32 of a-z, 0-9, '-' and '_'` / `'kokoro' is a built-in engine` |
| count | at most 16 profiles | `at most 16 engines` |
| `label` | 1 to 40 chars, no control chars | |
| `url` | absolute `http`/`https`; `https` required unless the host is loopback (`localhost`, `127.0.0.0/8`, `::1`) or `options.allow_http` is `true`; no userinfo, no query, no fragment | `a key is never sent over http to a non-loopback host` when `allow_http` is set together with a key_ref other than none on a non-loopback host |
| `model`, `voice` | at most 200 chars, no control chars | |
| `key_ref` | `env:` name `^[A-Za-z_][A-Za-z0-9_]{0,127}$`, ending in `_API_KEY` or `_SPEECH_KEY`, `SPEECH_KEY`, or starting with `SONARA_` (case-insensitive; review of #224: a client must not send other secrets of the runtime's environment) | `environment variable 'NAME' in key_ref must end in _API_KEY or _SPEECH_KEY, or start with SONARA_` |
| `options` | known keys of the kind only, typed (tables in 5.4); unknown keys are refused, so a typo is caught | `unknown option 'x' for kind 'openai-compatible'` |
| kind-specific | required fields per kind (5.4) | `kind 'azure' needs options.region or url` |

Default `key_ref` when absent: `none` for `openai-compatible` with a loopback `url` and for `command`; `credman` otherwise.

### 5.3 `engines.json` (home root)

```json
{
  "format": 2,
  "engines": [
    {"id": "openai", "kind": "openai-compatible", "label": "OpenAI", "url": "https://api.openai.com/v1",
     "model": "<model>", "voice": "<voice>", "key_ref": "credman", "options": {"preset": "openai"}},
    {"id": "kokoro-gpu", "kind": "openai-compatible", "url": "http://127.0.0.1:8880/v1",
     "voice": "af_heart", "key_ref": null, "options": {"preset": "kokoro-fastapi"}}
  ]
}
```

- Written atomically (temp file, rename) like `config.json`; missing file = no profiles; a file that is not valid JSON is copied to `engines.json.bad`, logged, and treated as empty until the next save. An entry that fails validation is logged, kept in the file and not registered.
- Never holds a key. A field named like a secret (`secret`, `api_key`, `key`) in an `engine_add` is never written (only `engine_key` stores secrets, in the KeyStore).
- Read once at start, before the reader starts, so a saved `engine` naming a profile works on the first sentence.
- Format 2 (review of #224): keys are bound to origins (6.4). An `env:` entry may hold `key_origin`, written only by the user (or the migration); `engine_add` never takes it from the client. A file of format 1 (or without `format`) is migrated at load and saved as format 2.

### 5.4 Kinds, URLs and options

Common options (every kind): `timeout_ms` (1000..=120000; default 15000 cloud, 60000 loopback and Gemini, 30000 command; in send mode `message` a request may take `Profile::answer_ms`: plus 70 ms per character), `prefetch` (1..=4; default 2 cloud, 1 loopback, command and Gemini), `allow_http` (bool), `chunk_chars` (200..=5000: the most characters per request in send mode `message`, when lower than the provider's limit; Gemini default 2000), `first_audio_ms` (1000..=60000, default 12000: the longest a streamed answer may go without audio, first and between pieces). Top-level `send_mode` (`message` or `sentence`, #235: "Send to the engine" below).

| Kind | `url` | `model` | `voice` | Kind options |
|---|---|---|---|---|
| `openai-compatible` | base URL with the API version, e.g. `https://api.openai.com/v1`, `http://127.0.0.1:8880/v1`. Default `https://api.openai.com/v1` with preset `openai` | none in code (#235): required for `openai`, `localai`, `speaches`; else optional (not sent) | none in code: required | `preset` (below), `response_format` (`wav` default, `pcm`), `sample_rate` (8000..=48000, used for raw PCM and sent where the preset supports it), `instructions` (string, sent only when the model accepts it), `extra` (JSON object merged into the request body last; for server-specific fields such as `{"stream": false}`), `voices_path` (override of the list path, `generic` preset) |
| `elevenlabs` | default `https://api.elevenlabs.io` | optional (not sent: ElevenLabs' own default); list `GET /v1/models` | required (voice_id) | `output_format` (`pcm_24000` default; `pcm_16000`, `pcm_22050`, `pcm_44100` (Pro tier)), `stability`, `similarity_boost`, `style` (0..=1), `language_code`, `enable_logging` (bool, default true: false only helps enterprise accounts) |
| `azure` | optional full endpoint base, e.g. `https://westeurope.tts.speech.microsoft.com`; else from `region` | none | required ShortName, from the voice list | `region` (required without `url`), `output_format` (`raw-24khz-16bit-mono-pcm` default; any `raw-*-16bit-mono-pcm` listed in 13), `lang` (xml:lang, default from the voice's locale prefix) |
| `google` | default `https://texttospeech.googleapis.com` | none (`model_name` option for Gemini TTS, flagged) | required, from the voice list | `language_code` (default from the voice name prefix), `sample_rate` (default 24000), `user_project` (sent as `x-goog-user-project`) |
| `gemini` (#235) | default `https://generativelanguage.googleapis.com` | required, none in code (letters, digits, `-`, `.`, `_`); list `GET /v1beta/models` (ids naming `tts`) | required, none in code; list `GET /v1beta/voices` (stored and prebuilt) | `language_code` (`speechConfig.languageCode`), `style` (at most 500 chars, `speechMetadata.style`); `chunk_chars` default 2000 and `first_audio_ms` as common options (the pre-release `quick_start` and `chunk_chars: 0` are folded into `send_mode`, #235). Default `timeout_ms` 60000 (plus 70 ms per character in send mode `message`); default `prefetch` 1 |
| `cartesia` | default `https://api.cartesia.ai` | required, none in code (typed; no list API used) | required voice id (uuid) | `api_version` (default `2026-08-14`), `language` (default `en`), `sample_rate` (8000, 16000, 22050, 24000 default, 44100, 48000) |
| `deepgram` | default `https://api.deepgram.com` (EU `https://api.eu.deepgram.com` by url) | the voice is the model: `voice` holds one of the `tts` models of the list; `model` unused | required | `sample_rate` (8000, 16000, 24000 default, 32000, 48000) |
| `command` | none | none | optional, substituted into `{voice}` (one of `voices` when that list is set; never starting with `-`; without the list a plain name of letters, digits, `_`, `-` and `.`, never a path) | `argv` (required, array, `argv[0]` an absolute path to an `.exe`, which must exist when the profile is added and before each start; `.bat` and `.cmd` refused; at most 64 entries; `{text}` refused: the text is never an argument), `input` (`stdin` default, or `file`: `{in}`, a temporary UTF-8 file with the text, must appear in argv), `output` (`stdout-wav` default, `stdout-pcm`, `file`: `{out}` must appear in argv), `sample_rate` (required for `stdout-pcm`), `voices` (array of strings shown as the voice list). **Local only**: never added or replaced over the protocol (`E_FORBIDDEN`, 14 PR3 security review) |

**Presets of `openai-compatible`** (`options.preset`, default `generic`):

Since #235 (user decision 2026-10-05) no preset has a default model or voice: the model is required for `openai`, `localai` and `speaches` and optional (not sent) for the others; the voice is always required (the voice rule, 14 "No model or voice in code"). The model list of every preset is `GET {url}/models`.

| Preset | Model | Key | Voice list | Notes |
|---|---|---|---|---|
| `openai` | required | required | none (OpenAI has no voice list API: typed, the page links OpenAI's guide) | `instructions` sent as set; a model that refuses it is asked without (`adapt`); input limit 4096 chars |
| `kokoro-fastapi` | optional | none | `GET {url}/audio/voices`, shapes `{"voices":["af_heart",...]}` or `{"voices":[{"id":..,"name":..}]}` | send `"stream": false` by default (correct WAV sizes); error body under `detail` |
| `localai` | required | optional | `GET {url}/audio/voices?model=<model>`, shape `{"data":[{"model":..,"voices":[{"name","language","gender"}]}]}` | always returns WAV; rate from the WAV header |
| `speaches` | required | optional | `GET {url}/audio/voices`, `{"voices":[{"id","name","language","gender"}]}` | sends `sample_rate` (default 24000) |
| `openedai-speech` | optional | none | none (typed) | archived project; generic target |
| `chatterbox-api` | optional | none | `GET {root}/voices` (not under `/v1`), `{"voices":[{"name","aliases","language"}]}` | always WAV, possibly 32-bit float (decode handles it); `speed` ignored by the server |
| `chatterbox-server` | optional | none | `GET {root}/get_predefined_voices`, `[{"display_name","filename"}]`, id = `filename` | `pcm` is refused (422), so `wav` is forced |
| `generic` | optional | per key_ref | `GET {url}/audio/voices` (or `voices_path`), any of the shapes above; an error or 404 gives an empty list | |

`{root}` is `url` with a trailing `/v1` removed.

## 6. Keys

### 6.1 API (`external/keys.rs`)

```rust
/// A secret value. Debug and Display print "[redacted]"; no Serialize; the buffer is
/// overwritten with zeros on drop (best effort, no extra crate).
pub struct Secret(String);
impl Secret { pub fn expose(&self) -> &str; }

pub struct StoredKey { pub secret: Secret, pub origin: Option<String> }

pub trait KeyStore: Send + Sync {
    fn get(&self, profile: &str) -> Result<Option<StoredKey>, KeyError>;
    fn set(&self, profile: &str, secret: &Secret, origin: &str) -> Result<(), KeyError>;
    fn delete(&self, profile: &str) -> Result<(), KeyError>;   // absent is Ok
    fn list(&self) -> Result<Vec<String>, KeyError>;           // profile ids that have a key
}

pub struct CredentialStore;     // Windows: CredReadW / CredWriteW / CredDeleteW / CredEnumerateW
pub struct MemoryStore;         // tests
pub struct FileStore(PathBuf);  // `sonarad --keys fake`: <home>\fake-keys.json, testing aid only

pub struct KeyResolver { store: Arc<dyn KeyStore> }
impl KeyResolver {
    /// The key for a profile now: None for KeyRef::None, the store's entry for CredMan,
    /// the process environment for Env(NAME). Read on every request (cheap), so a key
    /// set with engine_key applies to the next chunk. Only for the origin the key is
    /// bound to (6.4); otherwise a `no_key` error and nothing is sent.
    pub fn resolve(&self, profile: &Profile) -> Result<Option<Secret>, ExtError>;
}
```

### 6.2 Credential Manager details

- Type `CRED_TYPE_GENERIC`, `TargetName` `sonara:<profile id>`, `UserName` `sonara`, `Comment` `Sonara speech engine key, sent only to <origin>`, `Persist` `CRED_PERSIST_LOCAL_MACHINE` (per user, survives logoff, does not roam).
- The origin the key is bound to (6.4) is the credential attribute `sonara-origin` (UTF-8, at most 256 bytes), written with the secret in the same `CredWriteW`; the comment only shows it.
- Blob: the key's UTF-8 bytes, at most 2560 bytes (`CRED_MAX_CREDENTIAL_BLOB_SIZE`); longer is `E_BAD_REQUEST` `key too long`.
- `list` uses `CredEnumerateW` with filter `sonara:*`.
- Not available (non-Windows build): `KeyError::Unavailable`, so `credman` profiles report `no_key`.
- The environment variable of an `env:` ref is read from `sonarad`'s own environment (inherited from whoever started it: the hook launcher, so Claude Code's environment). `engine_list` reports `key_present` for it, so the user sees whether the runtime can see it.

### 6.3 Rules

- A secret enters only through `engine_key` (or `engine_add` with `secret`, which is stored the same way), from the protocol, the CLI (read from stdin or a no-echo prompt, never an argument) or the settings page (a password field).
- A secret leaves only in the provider's auth header (13), over https, or over http to a loopback host. Never in a URL (Google's `?key=` is not used; `X-goog-api-key` header instead).
- Never logged: `trace_log` drops `secret` from `in` lines of `engine_add` and `engine_key` (not only masks it); provider error bodies are clipped to 300 chars and passed through `sonara_log::secrets::mask` before they reach any message; `ExtError` never holds request headers.
- `sonara uninstall` deletes every `sonara:*` credential unless `--keep settings` (the default keep), together with `engines.json` handling.

### 6.4 Keys are bound to their origin (security review of #224)

Threat: a protocol client holding the runtime token replaces a profile, keeping its id, with a `url` (or `region`, host option) of its own server; the stored key would go there with the next request. Rule: a key is only ever sent to the origin it was entered for.

- Origin: `scheme://host:port` of the base URL in force (the port always written, IPv6 in brackets): the profile's `url`, else the kind's default (`Profile::origin`, `profile::origin_of` for entries of any kind, supported or not): the `openai-compatible` preset's (`openai` only), `https://api.elevenlabs.io`, `https://<region>.tts.speech.microsoft.com` (a region is `[a-z0-9]{1,40}`), `https://texttospeech.googleapis.com`, `https://api.cartesia.ai`, `https://api.deepgram.com`; for `command` its program, `command:<argv[0] in lower case>` (`profile::command_origin`; the arguments are not part of it), which is also its default origin since a `command` profile comes only from the local file (#226 lockdown).
- `credman`: the origin is stored with the secret (6.2). `KeyResolver::resolve` returns the key only when the stored origin equals the profile's origin; a mismatch or a missing origin is `ExtError(no_key, "the key of 'x' was entered for A, not B: enter the key again")`, `key_present` is false, the engine falls back as usual and nothing is sent. Never "bind on first use".
- Every edit path binds or deletes: `engine_add` with `secret` and `engine_key` store the key bound to the profile's origin now; `engine_add` without `secret` deletes a stored key whose origin is not the new profile's (a replace to another url or region, or a stale credential of a removed profile with the same id). `engine_reload` (#226), and the reload `engine_add`/`engine_remove` do first when the file changed on disk, delete a stored key whose origin is not its entry's in the file now (a hand edit or `sonara engines add --kind command` that moved an address or a program); a reload never binds a key (only the migration at start does). The CLI never lowers the file's `format` (a new file is 2), so a file the runtime already migrated is never migrated again.
- `env:NAME`: the variable is the user's, so there is nothing to delete; the key is sent only to the provider's default origin (`Profile::default_origin`), or to the entry's `key_origin` in `engines.json`, which only the local file sets: the user, or the migration below. `engine_add` never reads `key_origin` from the client, keeps the old one on a replace only when the variable and the origin stay the same, and so a `url` that arrives over the protocol must be confirmed in the file (`no_key` message names the line to add) before the key follows it.
- Migration (format 1 to 2, at load, before any engine is built): a credential without an origin is bound to its entry's origin in the file, and an `env:` entry without `key_origin` gets its origin, because the file is local and its addresses were the user's. The file is then saved as format 2, so a credential without an origin found later (written by an older runtime) is never bound and is refused.
- Transport (second review of #224): an engine's agent (`http::provider_agent`) follows no redirect, so a key header never goes to a host named in a `Location` (ureq strips only `Authorization` on a redirect; `xi-api-key`, `x-goog-api-key`, `Ocp-Apim-Subscription-Key`, `X-API-Key` would follow); a 3xx is an error and falls back. A loopback profile never uses a proxy (ureq ignores Windows' `<local>` bypass list, so a key for `http://127.0.0.1:<port>` would otherwise reach the system proxy in clear text); a non-loopback profile uses the system proxy (`HTTP(S)_PROXY`/`ALL_PROXY`, else Internet Settings), over https. `Url::parse` accepts only a host that the HTTP client reads the same way: an IPv6 literal alone in brackets (optionally `:port` after), else ASCII letters, digits, `.`, `-`, `_`. `key_allowed` checks the request URL without its query.
- Accepted residual risks: (1) a key bound to a loopback origin goes to whatever process listens on that port; while the user's local server is down, a local program can bind it and receive the key on the next request or `engine_test`, without changing the profile. Do not store a key for a local server you do not keep running (most local servers need none). (2) The origin has no path: on a host that serves several tenants by path (an API gateway, `/<tenant>/v1`), a replace that changes only the path keeps the key and sends it to the other route on that host. Both need the user's own machine or the user's chosen host to be hostile already.
- `--keys fake`: `fake-keys.json` holds `{id: {"key", "origin"}}` (a bare string, the old form, has no origin).
- Tests: `sonarad/tests/engines.rs` (a replace over TCP and HTTP to a second fake server never sends the key there; a key bound elsewhere or unbound is refused; voice and model changes keep it; a new secret in the same `engine_add` or a later `engine_key` works; env keys follow only a confirmed url), `engines.rs` unit tests (migration, format 2), `keys.rs` and `profile.rs` unit tests, `conformance/engines/test_engines.py`, and `credman_live` (the attribute round trip).

## 7. The `External` engine

### 7.1 Config

```rust
pub struct ExternalConfig {
    pub profile: Profile,
    pub keys: KeyResolver,
    pub fallback: Option<Arc<dyn Engine>>,     // sonarad: Kokoro (-> OneCore); fake runs: the fake engine
    pub fallback_voice: String,                // sonarad: "af_sarah" when the fallback offers it, else ""
    pub notice: Option<Arc<dyn Fn(Notice) + Send + Sync>>,   // sonarad: support log line
    pub clock: Arc<dyn Fn() -> Instant + Send + Sync>,       // tests inject
    pub agent: Option<ureq::Agent>,            // tests inject; default http::agent(timeouts)
}
pub struct Notice { pub engine: EngineId, pub reason: Option<Reason>, pub status: Option<u16>, pub message: String, pub fallback: Option<EngineId> }
// reason None: a recovery after failures ("engine <id> recovered"); fallback: the engine that spoke instead
```

`External::new(config) -> Result<External>` fails only on an invalid profile or an unsupported kind.

### 7.2 Engine methods

| Method | Behaviour |
|---|---|
| `id` | `EngineId::intern(profile.id)` |
| `license_class` | `External` |
| `voices` | last fetched list (cached in memory 10 min, also the preset's fixed list), plus the profile voice if not listed; never blocks; `installed: true` |
| `refresh_voices` | fetches per kind (13), timeout 10 s, updates the cache; an error is `Error::External` |
| `accepts_unlisted_voices` | `true` |
| `lookahead` | `options.prefetch` or its default |
| `warm` | no network call (a cold profile must not send text or spend quota); checks the key resolves when the kind needs one: a missing key sets the blocked state `no_key` (status), returns `Ok(())` when a fallback exists |
| `synthesize` | 7.3 |
| `cancel` | bumps a generation counter, wakes the waiting `synthesize` (returns `Cancelled` at once; the request thread finishes and its result is dropped), cancels the fallback |
| `status` | 8.4 |

### 7.3 Synthesis path

```
synthesize(text, voice, rate):
  voice := voice or profile.voice or preset default         (empty and none: BadConfig "no voice")
  if text.len() <= 64 chars and CueCache has (voice, rate, text): return it
  if Health says skip (breaker open or blocked):            fallback(text, rate, reason)   [no network]
  key := resolve() ; missing and needed:                    block(NoKey); fallback(...)
  parts := split(text, adapter.input_limit())
  for part in parts:  pcm += request(part)  on a worker thread, wait with cancel
      error e:  health.record_failure(e); notice(e); return fallback(text, rate, e.reason)
  health.record_success(); cache short text; return pcm as one PcmStream
fallback(text, rate, reason):
  no usable fallback: Err(Error::External{reason, message})   (the reader skips the chunk and logs it)
  cue := health.take_cue(reason) ? cue_text(reason, label) : none
  pcm := [cue via fallback, text via fallback]  (fallback voice, same rate)
```

- A chunk is never sent twice to the provider after a failure (no retry loop); the one exception is a 429 or 503 with `Retry-After` of at most 1.5 s, retried once after that wait (the wait is cancellable).
- Parts of a split chunk are requested in order; a failure of any part sends the whole chunk to the fallback (no mixed voices within a sentence).
- The audio of one request becomes one `PcmChunk` at the provider's rate (rodio resamples per buffer, so mixed rates across chunks are fine).

### 7.4 Latency: prefetch and cache

- Today the reader synthesizes the chunk after the playing one (`sonara-core` `prefetch`), on one synth thread, and plays a chunk only once it is fully synthesized. With `lookahead` 2 for cloud profiles, two chunks are requested ahead; a cloud round trip (typically 0.3 to 1.5 s) is then hidden behind the playing sentence(s). The synth thread stays single, so at most one request per engine is in flight: this also respects free-plan concurrency limits (ElevenLabs Free 2, Cartesia Free 2).
- First sentence of a new item while idle: no prefetch is possible; its latency is one round trip. Accepted for this feature. A later change could stream the first chunk (the HTTP bodies of OpenAI, ElevenLabs, Azure raw, Cartesia and Deepgram already arrive chunked) by letting the synth thread hand partial PCM to the output; out of scope here.
- `CueCache`: LRU of 128 entries keyed by `(voice, rate, text)` for texts of at most 64 chars, in memory only, only provider audio (never fallback audio). It serves the spoken cues of the `system` extension ("Paused.", "Rate 250.") and repeated short lines. Cleared on profile replace.
- Rate, voice changes apply to chunks synthesized from then on (unchanged rule); a deeper lookahead means up to two already-synthesized chunks keep the old setting.

### 7.5 Timeouts

`http::agent` per profile: connect 5 s, `timeout_recv_response` = `timeout_ms`, `timeout_recv_body` = `timeout_ms` (review of #224: was `timeout_ms` + 30 s, which meant 45 to 90 s of silence on a connection that stalls mid-body), `http_status_as_error(false)` (bodies of 4xx/5xx are read for the error mapping), max audio body 64 MiB, max error body 16 KiB.

## 8. Fallback, health and cues

### 8.1 Reasons and policy

| Reason | Comes from | Class | Effect |
|---|---|---|---|
| `no_key` | key_ref needs a key and none resolves | blocked | until `engine_key`, `engine_add` (replace) or a successful `engine_test`; re-checked at each chunk without a network call |
| `auth` | 401, 403 auth codes | blocked | as `no_key` |
| `quota` | quota/credit codes (402, OpenAI 429 quota codes, ElevenLabs 401 `quota_exceeded`) | blocked 10 min | then one probe chunk |
| `bad_voice` | unknown voice codes (13) | blocked | until a `set voice`, profile replace, or `engine_test` success. A `set voice` reaches the engine as a new voice argument: the block is per voice |
| `bad_config` | 400/404/415/422 not about the voice, unknown model, wrong region | blocked | as `auth` |
| `format` | 200 with a body that is not WAV/PCM (MP3, JSON, HTML) or an unsupported WAV | blocked | as `auth` |
| `rate_limited` | 429 (not quota), concurrency limits | transient | breaker |
| `network` | DNS, connect, TLS, reset | transient | breaker |
| `timeout` | any ureq timeout | transient | breaker |
| `server` | 5xx, 503 overloaded | transient | breaker |

**Breaker** (`health.rs`): two consecutive transient failures open it for 30 s, then 60, 120, 240, capped at 300 s; while open, chunks go straight to the fallback with no network wait; when it expires the next chunk is a probe; a success closes it and resets the backoff. One transient failure alone only sends that chunk to the fallback, unless it carries a `Retry-After` (or Gemini's `retryDelay`) longer than the one 1.5 s retry: then the breaker opens at once for that long, capped at 300 s (#235). A `quota` block lasts that wait when it is longer than 10 minutes, capped at 24 hours.

### 8.2 Cue

Spoken once per episode: when a reason first causes a fallback, and again only after a success (or a profile change) followed by a new failure. Text (`{label}` = profile label or the preset/kind display name):

| Reason | Cue |
|---|---|
| `no_key` | `{label} has no key. Reading with the built-in voice.` |
| `auth` | `{label} refused the key. Reading with the built-in voice.` |
| `quota` | `{label} is out of credit. Reading with the built-in voice.` |
| `rate_limited` | `{label} is busy. Reading with the built-in voice.` |
| `network`, `timeout` | `{label} cannot be reached. Reading with the built-in voice.` |
| `server` | `{label} has a server problem. Reading with the built-in voice.` |
| `bad_voice` | `{label} does not know this voice. Reading with the built-in voice.` |
| `bad_config`, `format` | `{label} settings do not work. Reading with the built-in voice.` |

### 8.3 Log

- `Notice` callback in `sonarad` writes to `logs\sonarad.log`, at most one line per (engine, reason) per 60 s: `engine <id> fallback reason=<reason>[ status=<http>] -> <fallback id>: <masked message>` (no text, no key, no URL query). A recovery: `engine <id> recovered`.
- The reader's existing readiness-change log line covers status changes (`engine 'openai' is not ready, retrying later: cannot reach api.openai.com; speaking with kokoro meanwhile`).
- Profile changes: `engine add id=<id> kind=<kind> host=<host>`, `engine remove id=<id>`, `engine key id=<id> set|cleared` (never the value).

### 8.4 `status()`

| State | `readiness` | `fallback` | `message` | `reason` |
|---|---|---|---|---|
| healthy or untried | `Ready` | none | none | none |
| breaker open | `Waiting` | fallback id | `cannot reach <host>` / `<host> is busy` / ... | transient reason |
| blocked `quota` | `Waiting` | fallback id | provider message (masked, 300 chars) | `quota` |
| blocked `no_key`, `auth`, `bad_voice`, `bad_config`, `format` | `Unavailable` | fallback id | message | reason |

`wire` adds `"reason": "<reason>"` to `engine_status` when set (additive field, protocol 1.2).

### 8.5 Muted: nothing is sent (#227, user requirement 2026-10-04)

"Make sure that API requests are never sent if Sonara is muted or super-muted." Sonara is **muted** while the reader is muted (core `control mute`, the mute hotkey without `agent`) or the agent's `mute_level` is 1 or 2 (also a saved one, from the moment the agent extension starts).

- **The hold** (`sonara-engine/src/external/hold.rs`): one `Hold` per runtime (`Engines::hold`), given to every profile's `External` (and to an unsaved profile's, for its voices) through `ExternalConfig::hold`. While held, `External::synthesize` sends nothing and speaks the chunk with its fallback (Kokoro, else OneCore; the fake engine in tests): not a failure, so no cue, no notice, no breaker or block. `refresh_voices` returns the last list known without a request. This one choke point covers every path: the reader's playing chunk and its lookahead, the "Session changed" announcements, a core `speak`, the spoken cues and the voice lists polled by the settings page.
- **Muting cuts what is in flight**: raising the hold moves every engine's cancel generation on, so a request in flight ends at once and its chunk is spoken locally; a chunk already `begin`-ed (lookahead) is spoken locally too. Nothing more is fetched or billed. `synthesize` reads the hold's epoch and the cancel generation *before* it checks the hold (`Hold::set` holds before it cancels), so a mute that lands between the two is either seen by the check or has moved the generation: no request slips through. Every `Hold::set` moves the epoch, so a request cut by a mute that already lifted again (a quick hotkey cycle) is still spoken locally, not dropped as a failed chunk ("nothing may silently drop it"); a reader cancel alone stays a cancel. A paged voice list checks the hold before each page and keeps no partial list.
- **The server** (`sonarad/src/quiet.rs`, `Quiet`): before a change that mutes (`set mute_level` 1 or 2, `control mute`, the mute hotkey) it raises the hold; after any mute change (and when the agent extension starts) it sets the hold to the truth (reader muted, or `mute_level` >= 1), so a failed change or an unmute releases it. Mute changes run one at a time (`Quiet::change`, a mutex held across the hold, the change and the sync), so a hotkey mute and a protocol unmute on two threads cannot leave a stale hold. A reader whose state cannot be read keeps the hold as it is.
- **Cues**: the mute transition cues ("Muted.", "Super muted.", "Unmuted.", core "Muted."/"Unmuted.") are always spoken with the local voice (`Cues::speak_local`, `hold::local`). "Unmuted." is said as the mute lifts and is still part of the muted episode, so the rule is simply: every cue of a mute transition is local. Any other cue queued while muted (a rate cue, "Paused.") is local too.
- **Explicit user actions stay allowed**: `engine_test` (the Test button; `External::test` ignores the hold) and `preview` (run inside `hold::explicit`). While the reader itself is muted (core `mute`) both are still sent but play silently, as every clip does then. A voice list is not one of them: while muted `voices` answers the known voices with `error: {reason: "muted", ...}` when a fetch was due (saved or unsaved profile).
- **Unmuted**, everything resumes with the next chunk.
- Tests: `crates/sonara-engine/tests/external_hold.rs`, `crates/sonarad/tests/muted_engines.rs` (a local provider that counts requests: zero while muted, super-muted, after muting mid-message, for the mute and rate cues; requests again after unmute; Test and preview reach it), `conformance/engines/test_muted_engines.py`.

## 9. Rate mapping (`rate.rs`)

`s = wpm / 200` (Sonara's range 100..=400 gives 0.5..=2.0; the product default 250 gives 1.25).

| Kind | Parameter | Mapping | Note |
|---|---|---|---|
| openai-compatible | body `speed` | `clamp(s, 0.25, 4.0)`, 2 decimals | ignored by Chatterbox servers |
| elevenlabs | `voice_settings.speed` | `clamp(s, 0.7, 1.2)` | Sonara 140..=240 wpm map exactly; above 240 stays at 1.2 (documented limit; no local time-stretch) |
| azure | SSML `<prosody rate="1.25">` | `clamp(s, 0.5, 2.0)` as a multiplier string | |
| google | `audioConfig.speakingRate` | `clamp(s, 0.25, 2.0)` | |
| cartesia | `generation_config.speed` | `clamp(s, 0.6, 1.5)` | |
| deepgram | query `speed` | `clamp(s, 0.7, 1.5)` | **uncertain**: range not in the fetched docs, likely Aura-2 only; omit the parameter when `s` is 1.0, and the live test checks a 1.25 request is accepted. If Deepgram refuses it, the adapter sends no speed (a `bad_config` on `speed` is retried once without it and remembered for the engine's life, until a restart or a profile replace) |
| command | `{rate}` (wpm), `{speed}` (`s`, 2 decimals) placeholders | as is | |
| gemini (#235) | `speechMetadata.style` (no speed parameter exists) | at most 160 wpm `speaking slowly`; 161..=280 nothing (the product default 250 sends no style); 281..=350 `speaking quickly`; above `speaking very quickly`; after the profile's `style`, joined with `, ` | approximate: a direction the model interprets. Never in the text (3.8 models read the text verbatim). A model that refuses `speechMetadata` gets no style from then on (`adapt`) |

## 10. Protocol (additions to `docs/protocol-v1.md`, protocol 1.2)

Capability `engines` in `hello.capabilities` and `runtime.json` when the host allows `External` (`sonarad` without `--no-external-engines`). Without it, every `engine_*` message is `E_UNSUPPORTED` (`this runtime does not allow external engines`). All five are core messages (no extension to enable) and work over TCP and HTTP (`POST /v1/engine_add`).

### 10.1 Profile view (in replies; never a secret)

```json
{"id": "openai", "kind": "openai-compatible", "label": "OpenAI", "url": "https://api.openai.com/v1",
 "model": "<model>", "voice": "<voice>", "key_ref": "credman", "options": {"preset": "openai"},
 "key_present": true, "sends_text_to": "api.openai.com", "local": false, "license_class": "external",
 "supported": true, "current": false,
 "status": {"ready": true, "status": "ready"}}
```

`sends_text_to`: the URL host; for `command`, `"program <file name of argv[0]>"`; `local`: loopback host or `command`. `status`: the `engine_status` object of this engine (without `engine`). `supported: false` for a kind this build lacks.

### 10.2 Messages

**`engine_list`** `{}` replies `{engines: [view...], builtin: ["kokoro", "onecore"], kinds: ["openai-compatible", ...], presets: ["openai", "kokoro-fastapi", ...]}` (`kinds` lists what this build implements).

```json
> {"type": "engine_list", "id": 1}
< {"id": 1, "ok": true, "engines": [{"id": "openai", ...}], "builtin": ["kokoro", "onecore"], "kinds": ["openai-compatible"], "presets": ["openai", "kokoro-fastapi", "localai", "speaches", "openedai-speech", "chatterbox-api", "chatterbox-server", "generic"]}
```

**`engine_add`** `{engine: {profile fields}, secret?: string, replace?: bool}` validates, saves `engines.json`, stores `secret` (forces `key_ref` `credman` when it was absent, `E_BAD_REQUEST` when it is `env:` or `none`), registers the engine and replies `{engine: view}`. It does not select it (cloud stays opt-in until `set engine`). An existing id without `replace: true` is `E_BAD_REQUEST` (`engine 'openai' exists; send replace: true`). A replace keeps the stored key unless `secret` is given, resets health and the cue cache, and, when it is the current engine, applies to the next chunk synthesized.

```json
> {"type": "engine_add", "engine": {"id": "openai", "kind": "openai-compatible", "options": {"preset": "openai"}}, "secret": "sk-..."}
< {"ok": true, "engine": {"id": "openai", "kind": "openai-compatible", "url": "https://api.openai.com/v1", "model": "<model>", "voice": "<voice>", "key_ref": "credman", "key_present": true, "sends_text_to": "api.openai.com", ...}}
> {"type": "set", "key": "engine", "value": "openai"}
< {"ok": true, "key": "engine", "value": "openai"}
```

A request's `id` is its correlation id (the reply echoes it; every SDK sets it), so the three messages below name the profile with **`engine`**, as `voices {engine}` does (changed in PR1: the first draft used `id`, which the SDKs overwrite).

**`engine_remove`** `{engine, forget_key?: bool (default true)}`: when it is the current engine, first `set engine` to the default choice (Kokoro when installed, else OneCore; saved like any `set`); then unregisters it from both registries, removes it from `engines.json`, deletes `sonara:<id>` when `forget_key`. Reply `{removed: "openai", engine: "<engine now in force>"}`. Unknown id: `E_NOT_FOUND`.

**`engine_key`** `{engine, secret: string | null}`: stores (or with `null` deletes) the profile's key in the KeyStore; clears `no_key`/`auth` blocks. A profile whose `key_ref` is `env:` or `none` is `E_BAD_REQUEST` (`engine 'x' reads its key from the environment variable NAME`). Reply `{engine, key_present}`.

**`engine_test`** `{engine, text?: string (default "Hello. This is how Sonara sounds with this voice.", at most 300 chars), voice?: string, play?: bool (default true)}`: one synthesis on that profile with **no fallback** and no cache, at the current rate; with `play`, played as a clip over whatever is read (as `preview`). Success clears the profile's blocked state and breaker. Reply `{ok: true, engine, voice, ms, sample_rate, duration_ms}`. Failure: `E_ENGINE` with the masked provider message and an additive `reason` in the error object:

```json
< {"ok": false, "error": {"code": "E_ENGINE", "message": "OpenAI refused the key (401): Incorrect API key provided", "reason": "auth"}}
```

**`voices`** (existing) gains `refresh?: bool`. With `engine` naming a profile, the runtime calls `refresh_voices` when its cache is older than 10 min or `refresh` is true (blocking that request up to 10 s, it runs in the handler's blocking task). On failure the reply is still `ok` with the voices known and an additive `error: {reason, message}`. Without `engine`, profiles contribute their cached lists only (no network). Voice objects of profiles have `"license_class": "external"`.

**`set voice`** with a profile as the current engine accepts any id (`accepts_unlisted_voices`), so an ElevenLabs or cloned voice id works without listing; `set engine` to a profile resets a voice it does not list (existing rule), so the profile's own `voice` applies.

### 10.3 Errors (existing codes)

| Case | Code |
|---|---|
| validation of a profile or a message field | `E_BAD_REQUEST` |
| unknown profile id | `E_NOT_FOUND` |
| host refuses external engines | `E_UNSUPPORTED` |
| Credential Manager failure, `engine_test` failure | `E_ENGINE` |

`docs/protocol-v1.md` also updates: Versioning (1.2 added `engines`, `engine_status.reason`, `voices.refresh`, `license_class` `external`), Saved settings (`engines.json` row, `config.json` `engine` may name a profile), Testing aids (`--keys fake`, `--no-external-engines`), the `sonarad.log` lines of 8.3, and a new section "External engines" with the kinds, presets, fallback and privacy notes.

## 11. CLI, SDKs, settings page

### 11.1 CLI (`sonara.exe`, PR1)

```
sonara engines list
sonara engines add <id> --kind openai-compatible [--preset openai] [--url U] [--model M] [--voice V]
                   [--key-env NAME | --no-key] [--option k=v ...] [--replace]
sonara engines key <id>            (reads the key from stdin, or prompts without echo when stdin is a console)
sonara engines key <id> --clear
sonara engines use <id>            (set engine)
sonara engines test <id> [text]
sonara engines remove <id> [--keep-key]
```

Thin protocol calls (`client.rs`). Never accepts a key as an argument.

### 11.2 SDKs (PR1)

TypeScript `clients/ts/src/engines.ts`, on `client.engines`:

```ts
export interface EngineProfile { id: string; kind: string; label?: string; url?: string; model?: string;
  voice?: string; key_ref?: string | null; options?: Record<string, unknown>; }
export class EnginesApi {
  list(): Promise<Reply>;                                                     // engine_list
  add(profile: EngineProfile, opts?: { secret?: string; replace?: boolean }): Promise<Reply>;
  remove(id: string, opts?: { forgetKey?: boolean }): Promise<Reply>;         // forget_key on the wire
  setKey(id: string, secret: string | null): Promise<Reply>;                  // engine_key
  test(id: string, opts?: { text?: string; voice?: string; play?: boolean }): Promise<Reply>;
}
```

Python `clients/python/src/sonara_client/engines.py`, `client.engines`: `list()`, `add(profile: dict, secret=None, replace=False)`, `remove(id, forget_key=True)`, `set_key(id, secret)`, `test(id, text=None, voice=None, play=True)`. Python 3.9 syntax, no dependencies. Both SDKs: `voices(engine, refresh=True)` pass-through. Docs: `docs/bundling.md` gains a short "External engines" paragraph (hosts may refuse with `--no-external-engines`).

### 11.3 Settings page (PR4)

Section **Engines** under Speech (`crates/sonarad/assets/settings.html`, served by `settings_page.rs`), using only the HTTP API:

- List: each profile with its label, kind, `sends_text_to` ("Sends text to api.openai.com" or "Runs on this PC"), key state ("Key saved" / "No key" / "Key from NAME"), status (ready, or the reason with the fallback), and buttons Use, Test, Edit, Remove (Remove asks for confirmation, then `engine_remove`). A `command` profile has no Edit: the page never adds or changes one (the runtime refuses it over the protocol anyway, `E_FORBIDDEN`), so its row says it is set up with `sonara.exe engines add` or in `engines.json`, the kind select of the form leaves `command` out, and a note under the list says how to add one.
- Add/edit form: kind (and preset for openai-compatible), label, url (prefilled per preset), model, voice (a select filled by `voices {engine, refresh: true}` after the profile exists, with a free-text "Other voice id" entry), key (password input, never prefilled, "Leave empty to keep the saved key"), advanced options per kind. Save = `engine_add` (with `replace` when editing) and `secret` when the key field is non-empty.
- Test button = `engine_test` with `play: true`; shows the time and the error with its reason.
- The engine picker that #214 removed stays removed: the Speech section shows "Kokoro (built in)" or the selected profile with a "Use Kokoro" button, as today for a non-Kokoro `engine`.
- Design per the user's rules (impeccable, taste-skill, ui-ux-pro-max, frontend-design, web-design-guidelines) and the page's existing style; accessible labels, keyboard operable, no key ever echoed into the DOM after saving.

## 12. Privacy and documentation

- `PRIVACY.md`: `engines.json` row (the engines you added: kind, address, model, voice, options; never keys); a row outside the home: Windows Credential Manager entries `sonara:<id>` (your keys; removed by `sonara uninstall` unless settings are kept); a section "External voices (opt-in)": when you select an external engine, the text Sonara reads (the same text it would speak) is sent to that provider, under the provider's terms; nothing is sent before you select one; a local server or program keeps it on your PC; the fallback cue and log name the reason only.
- `README.md` (PR4): Engines section: what a profile is, the table of kinds and presets, how to add a key (settings page, CLI), privacy line, the fallback behaviour, rate limits per provider (ElevenLabs 0.7 to 1.2).
- `docs/architecture.md`: one paragraph and the module list for `external/`.
- `CLAUDE.md`: the Current work line points at this document.

---

## 13. Provider facts (research 2026-10-04; uncertain points flagged)

### 13.1 Request, auth and audio per provider

| Provider | Synth request | Auth header | Audio Sonara asks for | Voices |
|---|---|---|---|---|
| OpenAI | `POST {url}/audio/speech` JSON `{model, input, voice, response_format: "wav", speed, instructions?}` | `Authorization: Bearer <key>` | WAV 24 kHz s16 mono (`pcm` option: raw 24 kHz s16le) | no list API (typed; #235 removed the fixed list); custom `voice_...` ids sent as `{"id": "voice_..."}`. Models: `GET {url}/models`, the ids naming `tts` |
| Kokoro-FastAPI | same path, `model` only when set, `stream: false` | none (any Bearer accepted) | WAV 24 kHz | `GET {url}/audio/voices` (two shapes) |
| LocalAI | same path, `model` required, `response_format: "wav"` | Bearer when configured | WAV at the backend's rate (from the header) | `GET {url}/audio/voices?model=` |
| Speaches | same path, `sample_rate: 24000` | Bearer when configured | WAV (streamed, placeholder sizes) | `GET {url}/audio/voices` |
| openedai-speech | same path | none | WAV | none (typed) |
| Chatterbox API (travisvn) | same path; `response_format` and `speed` ignored | none | WAV 24 kHz, possibly 32-bit float | `GET {root}/voices` |
| Chatterbox-TTS-Server (devnen) | same path, `voice` = file name | none | WAV int16 24 kHz | `GET {root}/get_predefined_voices` |
| ElevenLabs | `POST {url}/v1/text-to-speech/{voice_id}?output_format=pcm_24000` JSON `{text, model_id, voice_settings: {speed, stability?, similarity_boost?, style?}, language_code?}` (non-stream endpoint; whole body) | `xi-api-key: <key>` | raw s16le mono at the named rate | `GET {url}/v2/voices?page_size=100&next_page_token=`, loop until `has_more` false; id `voice_id`, name `name`, language from `labels.language` or `""` |
| Azure | `POST {base}/cognitiveservices/v1`, body SSML `<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{lang}'><voice name='{voice}'><prosody rate='{s}'>{escaped text}</prosody></voice></speak>` | `Ocp-Apim-Subscription-Key: <key>`, plus `Content-Type: application/ssml+xml`, `X-Microsoft-OutputFormat: raw-24khz-16bit-mono-pcm`, `User-Agent: Sonara/<version>` | raw s16le mono 24 kHz | `GET {base}/cognitiveservices/voices/list`; id `ShortName`, name `LocalName (Locale)`, language `Locale` |
| Google | `POST {url}/v1/text:synthesize` JSON `{input: {text}, voice: {languageCode, name}, audioConfig: {audioEncoding: "PCM", sampleRateHertz: 24000, speakingRate}}`; reply `{audioContent: base64}` | `X-goog-api-key: <key>` (**uncertain**: API keys are not on the TTS auth page; widely used; the live test is the acceptance check), `x-goog-user-project` when set | headerless s16le mono (`LINEAR16` would carry a WAV header; `PCM` is used) | `GET {url}/v1/voices?languageCode=`; id `name`, language `languageCodes[0]` |
| Cartesia | `POST {url}/tts/bytes` JSON `{model_id, transcript, voice: {"id": ...}, output_format: {container: "raw", encoding: "pcm_s16le", sample_rate: 24000}, language, generation_config: {speed}}` | `Authorization: Bearer <key>` and `Cartesia-Version: 2026-08-14` | raw s16le mono | `GET {url}/voices?limit=100&starting_after=`, loop on `has_more`/`next_page`; id `id`, name `name`, language from `accents[0].locale` or `language` |
| Deepgram | `POST {url}/v1/speak?model={voice}&encoding=linear16&container=none&sample_rate=24000[&speed=]` JSON `{text}` | `Authorization: Token <key>` | raw s16le mono at `sample_rate` | `GET {url}/v1/models`, the `tts` array; id `canonical_name`, language `languages[0]` |
| Gemini (#235) | `POST {url}/v1beta/models/{model}:streamGenerateContent?alt=sse` (server-sent `GenerateContentResponse` events; a model that refuses it: `:generateContent`, the whole answer) JSON `{contents: [{role: "user", parts: [{text, speechMetadata?: {style}}]}], generationConfig: {responseModalities: ["AUDIO"], speechConfig: {voiceConfig: {prebuiltVoiceConfig: {voiceName}}, languageCode?}, responseFormat: {audio: {mimeType: "AUDIO_L16", sampleRate: 24000}}}}` (a stored `voice_...`/`voicekey_...` id goes as `voiceConfig: {voice}`); reply `candidates[0].content.parts[].inlineData {mimeType, data: base64}` | `x-goog-api-key: <key>` (never `?key=`) | headerless s16le mono 24 kHz (`AUDIO_L16`; without `responseFormat` the 3.8 models answer WAV, older ones L16: both are read) | `GET {url}/v1beta/voices?page_size=1000` (paged `next_page_token`; stored voices, then the prebuilt catalog; `id`, `display_name`, `language_code`, camelCase read too). Models: `GET {url}/v1beta/models?pageSize=1000` (paged `nextPageToken`; `name` `models/<id>` naming `tts`, with `generateContent` in `supportedGenerationMethods`) |
| command | `argv` with placeholders, no shell, `CREATE_NO_WINDOW`; text on stdin (UTF-8, then closed) or in a temp file at `{in}`, never in argv; env `SONARA_ENGINE_KEY` when a key resolves | n/a | WAV on stdout, raw s16le on stdout at `sample_rate`, or a WAV file at `{out}` (a temp path, deleted after) | `options.voices` |

Input limits (`split.rs`): OpenAI 4096 chars; ElevenLabs 5000 chars (the smallest per-model limit, v3; flash models allow more, but a sentence chunk never needs it); Azure 2000 chars per request (Sonara's choice, well under the 10-minute audio cap); Google 5000 UTF-8 bytes of the text (SSML not used); Cartesia 2000 chars (no documented limit; Sonara's choice); Gemini 4000 chars (the models take 8192 input tokens; a whole message is at most `chunk_chars`, 2000 by default, per request); Deepgram 2000 chars; Chatterbox API 3000 chars; others 4096 chars.

Audio parsing (`audio.rs`): `RIFF....WAVE` goes through `wav::decode` (handles float, LIST, placeholder sizes); otherwise raw s16le at, in order, the `Content-Type` `rate=` parameter, the requested rate, 24000; an odd trailing byte is dropped; a body starting with `ID3`, a valid MP3 frame header (sync, version and layer not reserved, bitrate index not 15, sample-rate index not 3), `OggS`, `fLaC`, `{` or `<` with a 200 status is `format` (`the server sent MP3, not WAV/PCM; set response_format`). When the adapter asked for raw PCM (`Adapter::raw_pcm`: OpenAI `pcm`, ElevenLabs, Azure, Google) the magic numbers are not sniffed, only a `Content-Type` naming mpeg, ogg/opus or flac is `format`: near-silent speech starts with sample -1 (`FF FF`), an MP3 frame sync, and a `format` error blocks the engine.

### 13.2 Error mapping per provider

Every adapter first checks the status; a 2xx body is audio, anything else is never played. Transport errors (DNS, connect, TLS, reset) are `network`; ureq timeouts are `timeout`. Messages come from the body fields named below (clipped, masked), else the status text.

**OpenAI and OpenAI-compatible** (body `{"error": {message, type, code}}`; Kokoro-FastAPI and Speaches `{"detail": {...} | [..] | "..."}`; Chatterbox API `{"detail": {"error": {message, type}}}`; LocalAI `{"error": {code, message, type}}`):

| Status / code | Reason |
|---|---|
| 401 | `auth` |
| 403 | `auth` (unsupported region, IP not allowlisted) |
| 429 with code `credit_balance_exhausted`, `organization_spend_limit_exceeded`, `project_spend_limit_exceeded`, `organization_usage_limit_exceeded`, `insufficient_quota` | `quota` |
| 429 otherwise (`slow_down`, LocalAI `rate_limit_error`) | `rate_limited` (honour `Retry-After` per 7.3) |
| 400, 404, 422 whose message or `param` mentions `voice` (also Chatterbox-TTS-Server 404 `Voice file ... not found`) | `bad_voice` |
| 400, 404, 415, 422 otherwise | `bad_config` |
| 500, 502, 504 | `server` |
| 503 (`server_is_overloaded`, LocalAI model loading) | `server` (honour `Retry-After` per 7.3) |
| 200 with a stream that ends early (Kokoro-FastAPI after an error mid-stream) | the audio received is used; an empty body is `format` |

**ElevenLabs** (body `{"detail": {type, code, message, status, request_id, param}}`, or 422 `detail` array):

| Status / code | Reason |
|---|---|
| 401 `invalid_api_key`, `missing_api_key` | `auth` |
| 401 with `status` `quota_exceeded` (older shape) | `quota` |
| 402 `insufficient_credits` | `quota` |
| 403 `voice_access_denied` | `bad_voice` |
| 403 `feature_not_available`, `subscription_required`, `model_access_denied` | `bad_config` (message suggests `pcm_24000` when the output format was the cause) |
| 404 `voice_not_found`, 400 `invalid_voice_id` | `bad_voice` |
| 404 `model_not_found`, 400 `unsupported_model`, `invalid_output_format`, `invalid_voice_settings`, `text_too_long`, `empty_text`, `invalid_text`, 422 | `bad_config` |
| 429 `rate_limit_exceeded`, `concurrent_limit_exceeded`, `system_busy` | `rate_limited` |
| 500 `internal_error`, 503 `service_unavailable`, `maintenance` | `server` |

**Azure** (error bodies are not documented as JSON: status only, body as opaque text, often empty):

| Status | Reason |
|---|---|
| 401 | `auth` (also a region that does not match the key: the message says "check the region") |
| 400 | `bad_config` (invalid SSML or header; a voice name Azure does not know also answers 400: when the voice is not in the cached list, `bad_voice`) |
| 415 | `bad_config` |
| 429 | `rate_limited` (quota or per-voice regional capacity; Microsoft advises backoff) |
| 502, 503, 5xx | `server` |

**Google** (body `{"error": {code, message, status, details}}`):

| Status / status | Reason |
|---|---|
| 400 `INVALID_ARGUMENT` mentioning the voice name | `bad_voice` |
| 400 `INVALID_ARGUMENT` otherwise | `bad_config` |
| 400 `API_KEY_INVALID` in details, 401 `UNAUTHENTICATED` | `auth` |
| 403 `PERMISSION_DENIED` (API not enabled, billing off, key restricted) | `auth` (message keeps Google's text, which names the fix) |
| 429 `RESOURCE_EXHAUSTED` | `quota` when the message mentions quota per day/billing, else `rate_limited` |
| 500 `INTERNAL`, 503 `UNAVAILABLE` | `server` |

**Gemini** (#235; generateContent errors are google.rpc.Status `{"error": {code, message, status, details}}`, with `RetryInfo.retryDelay` and `QuotaFailure.violations[].quotaId` on 429; that body shape comes from community reports, not the TTS docs):

| Status / status | Reason |
|---|---|
| 400 with `API_KEY_INVALID` (or "API key not valid"), 401, 403 `PERMISSION_DENIED` | `auth` |
| 400 whose message says a voice is not found, does not exist, or is invalid | `bad_voice` |
| 400 naming an unknown field `responseFormat` or `speechMetadata` (`Unknown name "..."`) | `bad_config` with `refused_param`; the part is sent once more without the field, which is never sent again (`adapt`) |
| 400 otherwise (`INVALID_ARGUMENT`, `FAILED_PRECONDITION`: billing, a region the API does not serve), 404 `NOT_FOUND` (the model; the message says to check it) | `bad_config` |
| 402 (Interactions `payment_required`) | `quota` |
| 429 `RESOURCE_EXHAUSTED` with a `...PerDay...` quota id | `quota`, blocked until the next 08:00 UTC (midnight PST; Google resets requests per day at midnight Pacific), at least an hour (`gemini::daily_wait`); never retried at once |
| 429 with a message about a daily or spend limit and no per-day quota id | `quota` (blocked for `max(10 min, retryDelay)`) |
| 429 otherwise (per minute; Google's message also says "check your plan and billing details", which is not taken as quota) | `rate_limited` (breaker open for the `retryDelay`) |
| 500, 503 `UNAVAILABLE` (overloaded), 504 | `server` |
| 200 without audio (`promptFeedback.blockReason`, a `finishReason` such as `SAFETY` or `OTHER`, text instead of audio) | `server` (transient: that sentence reads with the fallback; the engine is not blocked) |

**Cartesia** (body `{"error_code", "title", "message", "request_id"}` for version 2026-03-01 and later; older versions `Title: Message` text):

| Status / error_code | Reason |
|---|---|
| 401 | `auth` |
| 402 `quota_exceeded` | `quota` |
| 403 `plan_upgrade_required` | `bad_config` |
| 404 `voice_not_found`, 422 `voice_model_mismatch` | `bad_voice` |
| 404 `model_not_found`, 422 `language_not_supported`, 400 (unsupported `Cartesia-Version`) | `bad_config` (message suggests updating `api_version`) |
| 429 `concurrency_limited` | `rate_limited` |
| 5xx | `server` |

**Deepgram** (body `{"err_code", "err_msg", "request_id"}` or `{"err_code", "message", "details"}`):

| Status | Reason |
|---|---|
| 401, 403 | `auth` |
| 402 | `quota` (**uncertain**: not in the fetched docs; mapped by convention) |
| 400 naming `model` | `bad_voice` (the voice is the model) |
| 400 otherwise, 413 (text over 2000 chars; should not happen after `split`), 422 | `bad_config` |
| 429 | `rate_limited` |
| 5xx | `server` |

**command**:

| Outcome | Reason |
|---|---|
| `argv[0]` missing or not runnable | `bad_config` |
| exit code not 0 | `server` (transient: a crash of the user's program), stderr's last line (masked) in the message |
| no output, or output not WAV/PCM | `format` |
| over `timeout_ms` | `timeout` (process killed) |

### 13.3 Uncertain facts (to verify with the live tests before each PR's "merge?")

1. Google API-key auth (`X-goog-api-key`) for `text:synthesize` is not on the official TTS auth page. Still open after PR2 (2026-10-04): `google_live` exists but was not run (no key on the build machine); it decides this at the PR's hands-on step.
2. Deepgram `speed` range and model support; Deepgram 402 for exhausted credit. Still open after PR3 (2026-10-04): `deepgram_live` exists but was not run (no key on the build machine); it prints whether a 250 wpm request comes back shorter than a 200 wpm one.
3. Cartesia: whether `/tts/bytes` starts sending before synthesis ends (no effect on this design, which collects the body).
4. Chatterbox API non-streaming WAV sample format (float32 expected; `wav::decode` handles both).
5. ElevenLabs model availability per account (superseded by #235: no default model; the model is optional and ElevenLabs picks its own when none is sent).
6. Azure 400 versus another code for an unknown voice name (the cached-list check covers both).
7. Azure voice list on a resource host (`<resource>.cognitiveservices.azure.com`): Sonara asks `/tts/cognitiveservices/voices/list` there (researched docs, 2026-10-04), the root path on a regional host. Not verified live: `azure_live` uses a region.
8. ElevenLabs: whether a partial `voice_settings` (only `speed`, or one of `stability`/`similarity_boost`/`style`) keeps the voice's stored values for the fields it leaves out. Sonara leaves `voice_settings` out when nothing is set at speed 1.0 (2026-10-04); `elevenlabs_live` should compare at the hands-on step.
9. Gemini (#235): the TTS body follows the official guide and API reference of 2026-10-05; not run live (no key on the build machine). `gemini_live` (`GEMINI_API_KEY`) decides that `responseFormat` with `AUDIO_L16` is accepted by the model the user picks, that `speechMetadata` (camelCase; the guide writes `speech_metadata`, proto JSON takes both) carries the style, and that `prebuiltVoiceConfig.voiceName` still selects a voice. A refusal of either field is handled (`adapt`, up to `MAX_ADAPTS` 2 per request), so each wrong guess costs one request, once.
10. Gemini rate limits per model (RPM, RPD) are not published; they show only in AI Studio. The default `chunk_chars` 2000 (send mode `message`) is Sonara's choice, not a documented optimum: about two minutes of speech per request, inside the models' output limit.
11. Gemini error bodies for generateContent (google.rpc.Status with `RetryInfo`) come from community reports; the docs name only 429 `RESOURCE_EXHAUSTED` and 503 `UNAVAILABLE`.

---

## 14. Implementation plan

> For agentic workers: one implementer per PR in its own worktree (`.claude/worktrees/<name>`), a reviewer, then a finalize step. Steps use `- [ ]`. Bug fixes and behaviours are test-first. Every PR runs all gates of `CLAUDE.md` (cargo fmt, clippy `-D warnings`, `cargo test --workspace`, `cargo deny check licenses bans`, `gen_notices.py --check`, build, `pytest conformance`, ruff, `pytest -q`, SDK tests when `clients/` changed, e2e when the settings page changed), bumps every version file listed in `CLAUDE.md` together, and never merges. Commits `type(scope): subject (#issue)` with the session trailer.

### PR1 (#224, 0.15.0): core, profiles, keys, fallback, prefetch, openai-compatible

**Files:** `crates/sonara-engine/{Cargo.toml, src/lib.rs, src/types.rs, src/registry.rs, src/error.rs, src/http.rs, src/kokoro/download.rs, src/external/{mod,profile,keys,error,health,cache,audio,rate,split,worker,adapter,openai}.rs, tests/common/mod.rs, tests/{registry,external_core,external_openai,external_live,credman_live}.rs}`, `crates/sonara-core/src/reader/mod.rs` (+ tests), `crates/sonara-reader/src/{worker,settings,lib}.rs` (+ tests), `crates/sonara-log/src/secrets.rs`, `crates/sonarad/src/{engines,engines_ext,protocol,args,main,wire,trace_log,support_log}.rs`, `crates/sonara-cli/src/{main,client,uninstall}.rs` (+ an `engines.rs`), `clients/ts/src/{engines,index,client}.ts` (+ tests), `clients/python/src/sonara_client/{engines,client,__init__}.py` (+ tests), `conformance/engines/{conftest,fake_openai,test_engines}.py`, `docs/protocol-v1.md`, `docs/architecture.md`, `docs/bundling.md`, `PRIVACY.md`, `THIRD_PARTY_NOTICES.md` (generated), version files.

- [ ] 1. `LicenseClass::External`, `Reason`, `EngineStatus.reason`, `EngineId::intern`: tests `intern_returns_the_same_static_str`, `external_class_is_refused_by_the_default_registry`.
- [ ] 2. Registry interior mutability, `unregister`, `replace`: tests `register_and_unregister_while_shared`, `replace_keeps_one_entry`, `unregister_unknown_is_unknown_engine`. Fix callers (`register(&self)`).
- [ ] 3. Engine trait defaults (`lookahead`, `accepts_unlisted_voices`, `refresh_voices`); reader core `set_lookahead` and multi-chunk prefetch: state-machine tests `lookahead_two_requests_two_ahead`, `lookahead_crosses_item_boundary`, `lookahead_change_while_playing_requests_more`, `rapid_controls_with_lookahead_three` (no duplicate `Synthesize`, none after an item ended). Worker sets it on init and switch (test with a fake engine of lookahead 2).
- [ ] 4. `resolve_voice` accepts unlisted ids for engines that allow it (test `unlisted_voice_accepted_by_open_engine`, `unlisted_voice_refused_by_kokoro`).
- [ ] 5. Move the ureq agent builder to `http.rs` (Kokoro download unchanged; its tests still pass).
- [ ] 6. `profile.rs`: parse/validate/serialize, presets table; table-driven tests for every row of 5.2 and the defaults of 5.4; round trip `engines.json` with an unknown kind kept.
- [ ] 7. `keys.rs`: `Secret` (Debug redacted test), `MemoryStore`, `FileStore`, `CredentialStore`, `KeyResolver` (credman, env, none). `credman_live.rs` `#[ignore]`: set, get, list, delete `sonara:test-<pid>`.
- [ ] 8. `error.rs` + `adapter.rs` shared body parsing; `openai.rs` mapping: table-driven test over every row of 13.2 (OpenAI-compatible), with body fixtures for each server shape.
- [ ] 9. `audio.rs`: tests `riff_with_list_chunk`, `streamed_wav_placeholder_sizes`, `float32_wav`, `raw_pcm_rate_from_content_type`, `odd_trailing_byte_dropped`, `mp3_body_is_format_error`, `json_body_is_format_error`.
- [ ] 10. `rate.rs` (every row of 9 at 100, 200, 250, 400 wpm), `split.rs` (chars and UTF-8 bytes, CJK, emoji, no split inside a word unless a word is longer than the limit).
- [ ] 11. `health.rs` with an injected clock: `two_transient_failures_open_the_breaker`, `backoff_doubles_to_300s`, `probe_after_expiry_closes_on_success`, `quota_blocks_ten_minutes`, `auth_blocks_until_cleared`, `cue_once_per_episode`.
- [ ] 12. `cache.rs`: LRU size and key tests; only provider audio cached.
- [ ] 13. `worker.rs` + `External::synthesize`: against the scripted server in `tests/common` (extend it: POST routes, status, headers, body, delay, captured requests). Tests in `external_core.rs`/`external_openai.rs`: request body golden per preset (model, voice, `response_format`, `speed`, `instructions` as set (#235: a model that refuses it is asked without), never for kokoro-fastapi or the Chatterbox presets, as set for the others, `stream: false` for kokoro-fastapi, `sample_rate` for speaches, `extra` merged last); `Authorization` sent only with a key and never over http to a non-loopback host; `cancel_returns_promptly_while_the_server_delays` (< 200 ms); `failure_speaks_the_chunk_with_the_fallback_and_prepends_the_cue_once`; `breaker_open_skips_the_network`; `retry_after_once_then_fallback`; `no_fallback_is_an_external_error`; `split_parts_failure_uses_fallback_for_the_whole_chunk`; `voices_per_preset` (both Kokoro-FastAPI shapes, LocalAI, Speaches, Chatterbox both, generic 404 = empty).
- [ ] 14. `sonara-log` secrets additions with tests.
- [ ] 15. `sonarad`: `--keys windows|fake`, `--no-external-engines`; `engines.rs` loads `engines.json` before the reader, registers profiles in the reader's and the previews' registries with Kokoro (fake: the fake engine) as fallback and `af_sarah` fallback voice; notice sink to `sonarad.log` (rate-limited); handlers of 10.2; `voices` refresh; capability `engines`, `PROTOCOL_MINOR` 2; wire `reason`, `license_class` `external`. Unit tests in `protocol.rs` style: each message's success and every error row of 10.3; `engine_remove_of_the_current_engine_switches_first`; `secret_never_in_trace_log_or_engines_json`.
- [ ] 16. CLI `sonara engines ...` (11.1) and uninstall removing `sonara:*` unless settings are kept (with a fake store in tests).
- [ ] 17. SDKs: TS `EnginesApi` + unit tests (wire shape) + integration test against `sonarad --engine fake --keys fake` with a local fake OpenAI server; Python the same.
- [ ] 18. Conformance `conformance/engines/`: a Python fake OpenAI server (thread, `http.server`), `sonarad --engine fake --keys fake`: `test_capability_engines_listed`, `test_add_list_remove_round_trip`, `test_speak_with_profile_hits_the_server_with_bearer`, `test_server_down_item_finishes_by_fallback_and_status_has_reason`, `test_profile_and_engine_survive_restart`, `test_secret_not_in_any_home_file`, `test_no_external_engines_flag_refuses`, `test_engine_test_reports_auth_reason`.
- [ ] 19. Live `external_live.rs` (`#[ignore]`): `openai_live` (`OPENAI_API_KEY`), `kokoro_fastapi_live` (`SONARA_LIVE_KOKORO_FASTAPI_URL`), `localai_live` (`SONARA_LIVE_LOCALAI_URL`, `SONARA_LIVE_LOCALAI_MODEL`), `speaches_live` (`SONARA_LIVE_SPEACHES_URL`, `SONARA_LIVE_SPEACHES_MODEL`). Each skips with a message when its variables are unset.
- [ ] 20. Docs: `protocol-v1.md` (10), `architecture.md`, `bundling.md`, `PRIVACY.md` (12); `CLAUDE.md` build line: the live test commands. Version 0.15.0 everywhere; notices regenerated.
- [ ] 21. Hands-on: branch build deployed per the safe redeploy steps, a Kokoro-FastAPI or OpenAI profile added with the CLI, a real Claude turn read, the network pulled mid-turn (fallback cue heard once), then "merge?".

**Deviations found while building PR1** (the sections above are updated where they apply):

- `engine_remove`, `engine_key`, `engine_test` name the profile with `engine`, not `id` (10.2): `id` is the request's correlation id, which the TS and Python SDKs and `sonara.exe` set on every request.
- `Notice` carries `reason: Option<Reason>` (`None` is a recovery) and `fallback` (7.1), so the host can write both log lines of 8.3 from one callback.
- The engine crate masks provider messages with `sonara_log::mask` (the leaf crate `sonara-log` becomes an optional dependency of feature `external`; it has no dependencies, so R7 holds).
- The product default voice (`af_sarah`) is applied at start and after `set engine` only when the engine lists it: an external engine accepts any voice id, so the default would otherwise be sent to the provider as a voice (a `bad_voice` on every first sentence). A user's saved voice still applies.
- `sonarad --engine fake` keeps a saved external engine (with the fake engine as its fallback), so conformance can check that a profile survives a restart; any other `--engine` still wins over the saved one.
- `SystemHost.previews` is `Option<Arc<Registry>>` (was `Option<Registry>`) so profiles are added to the previews' registry while the runtime runs; `sonara_reader::Config::registry` is `Arc<Registry>` (`Config::new` takes either) and `ReaderHandle::registry()` returns it.
- `sonara.exe` gains a `windows` dependency (Credential Manager for `uninstall`, the console mode for the key prompt without echo).
- The reader test of 4 is `unlisted_voice_refused_by_a_listed_only_engine` (the reader's tests have no Kokoro; the fake engine lists its voices like Kokoro).
- Prefetch with a deeper lookahead also runs while a loaded chunk is paused (the existing one-ahead rule already did).
- Review of PR1: `engine_test` runs the provider round trip outside the admission lock (only its play is admitted), so a slow provider never holds up `speak` or `control`; `Engine::begin` (default nothing) lets the reader mark a chunk under its queue lock so a cancel before `synthesize` starts still ends it; a profile blocked at `warm` (no key) logs its `fallback` line at the episode's first fallback; `env:` key refs are limited to API-key names; `instructions` is sent per preset (not only to `gpt-4o-mini-tts`); `timeout_recv_body` is `timeout_ms`.
- Step 21 (hands-on with a deployed branch build) is left for the "merge?" step: the PR1 worker must not touch `%LOCALAPPDATA%\Sonara` or the running runtime.

### PR2 (#225, 0.16.0): elevenlabs, azure, google

- [ ] 1. `base64` dependency (lock version), notices regenerated, `cargo deny` clean.
- [ ] 2. `elevenlabs.rs`: request golden (path with voice id, `output_format` query, `voice_settings.speed` clamp), `xi-api-key`, pagination of `/v2/voices` (fake server with three pages), mapping table test for every row of 13.2.
- [ ] 3. `azure.rs`: SSML builder with XML escaping (`&`, `<`, `>`, `'`, `"`; test with each and with non-ASCII), `xml:lang` from the voice prefix, headers (incl. `User-Agent`), region versus url, voice list parsing, mapping table test.
- [ ] 4. `google.rs`: request golden (`audioEncoding: PCM`, `sampleRateHertz`, `speakingRate`), base64 decoding of `audioContent`, 5000-byte split, voice list, mapping table test incl. `API_KEY_INVALID`.
- [ ] 5. Kinds registered in `engine_list.kinds`; conformance: one fake server per shape (ElevenLabs, Azure, Google) in `conformance/engines/fakes.py` with a speak and an auth-failure case each.
- [ ] 6. Live tests: `elevenlabs_live` (`ELEVENLABS_API_KEY`, optional `SONARA_LIVE_ELEVENLABS_VOICE`), `azure_live` (`AZURE_SPEECH_KEY`, `AZURE_SPEECH_REGION`), `google_live` (`GOOGLE_TTS_API_KEY`): each speaks one sentence and lists voices. The Google live test decides uncertain fact 1; its result goes into this spec's 13.3 with the date.
- [ ] 7. Docs (`protocol-v1.md` kinds, `PRIVACY.md` providers), version 0.16.0, hands-on with at least one cloud provider the user has a key for, "merge?".

**Deviations found while building PR2** (the sections above are updated where they apply):

- Branch `feat/225-cloud-adapters` (the table said `feat/225-cloud-engines`).
- `adapter::key_allowed` stripped nothing, so a request URL with a query (ElevenLabs' `?output_format=`) never got its key (`Url::parse` refuses a query). It now judges the URL without its query; test in `adapter.rs`.
- `Profile::default_key_ref` follows 5.2 literally: `none` only for an `openai-compatible` profile with a loopback url (PR1 gave `none` to any kind with a loopback url, so a cloud kind behind a local proxy would have sent no key).
- The `Adapter` trait grew three things: `map_error(reply, voice, listed)` (`listed`: whether the voice is in the last fetched list, `None` before one; Azure's 400 needs it), `audio(reply, label)` (default: the body is WAV or raw PCM; Google decodes base64 JSON), and `next_voices_page(body, key)` (default none; ElevenLabs' `next_page_token`, at most 50 pages). `External::refresh_voices` follows the pages and drops duplicate ids.
- Mapping rows the tables of 13.2 leave open: ElevenLabs 403 with another code is `auth`, a 404 whose message names the voice is `bad_voice`; Azure 403 is `auth`, a 400 before any voice list is fetched is `bad_config` (the message says to check the voice and language), and a 401 without a body says to check that the key belongs to the region; Google 429 is `quota` only when the message mentions "per day" or "billing".
- Kind options are validated like `openai-compatible`'s: ElevenLabs `output_format` one of `pcm_16000`, `pcm_22050`, `pcm_24000`, `pcm_44100`, `stability`/`similarity_boost`/`style` numbers 0..=1; Azure `region` a-z and 0-9, `output_format` one of the six `raw-{8khz,16khz,22050hz,24khz,44100hz,48khz}-16bit-mono-pcm` (13 listed none); Google `sample_rate` 8000..=48000, `user_project` a project id. `azure` and `google` refuse a `model` (Google's message points at `options.model_name`). Each of the three needs a `voice`.
- Labels when the profile has none: `ElevenLabs`, `Azure Speech`, `Google Text-to-Speech` (spoken in cues).
- Google: `languageCode` is the option, else the voice name's locale, else `en-US` (a voice name without a locale prefix has none); the voice list asks `?languageCode=` only when the option is set; `model_name` is sent as `voice.modelName` and is not verified live.
- Azure: `xml:lang` from `options.lang`, else the voice's locale, else `en-US`; the voice name is XML-escaped too (an attribute); control characters other than tab and newlines become spaces (XML 1.0 forbids them).
- Conformance: `conformance/engines/fakes.py` (one fake per shape, checking the key header) and `test_cloud_engines.py` (speak with the key in the provider's header, voices, `engine_test`; a refused key reports `auth` and the fallback reads).
- Live tests were written but not run (no provider keys on the build machine); step 7's hands-on decides them.
- Review fixes (2026-10-04): raw PCM that an adapter asked for is not sniffed for MP3/Ogg/FLAC magic numbers (a first sample of -1 made a sentence `format` and blocked the engine; this also fixes PR1's `pcm` path), and the generic MP3 sniff needs a valid frame header; the Azure voice list on a resource host is under `/tts`; ElevenLabs leaves `voice_settings` out when nothing is set at speed 1.0; a Google 400 is `bad_voice` only when the message says the voice does not exist or is not found (a feature the voice lacks is `bad_config`); `sonara-log` masks `sk_` keys (ElevenLabs).

### PR3 (#226, 0.17.0): cartesia, deepgram, command

- [ ] 1. `cartesia.rs`: headers (`Cartesia-Version` from options), body golden, cursor pagination, mapping table test.
- [ ] 2. `deepgram.rs`: query golden (`encoding=linear16&container=none&sample_rate=`), `Token` auth, `/v1/models` `tts` parsing, speed handling of 9 (retry once without `speed` on a `bad_config` about it, remembered), mapping table test.
- [ ] 3. `command.rs`: a test helper binary `src/bin/sonara-fake-tts.rs` with `required-features = ["test-util"]` (reads stdin, writes a WAV or raw PCM or a file per flags, can sleep, exit non-zero or print garbage); tests use `env!("CARGO_BIN_EXE_sonara-fake-tts")`. If Cargo does not build a feature-gated bin for the crate's own integration tests, move the helper into a small unpublished `crates/sonara-test-tools` crate used as a dev-dependency. Tests: stdin and `{text}` input, the three outputs, placeholders, timeout kills the process, cancel kills it, non-zero exit is `server`, garbage is `format`, temp file removed, no shell (an argument with `&` reaches the program verbatim).
- [ ] 4. Conformance fakes for Cartesia and Deepgram; a `command` profile with the helper binary.
- [ ] 5. Live: `cartesia_live` (`CARTESIA_API_KEY`, `SONARA_LIVE_CARTESIA_VOICE`), `deepgram_live` (`DEEPGRAM_API_KEY`; also decides uncertain fact 2), `command_live` (`SONARA_LIVE_COMMAND`: a JSON argv of a real local program, for example a Piper install of the user's own).
- [ ] 6. Docs, version 0.17.0, hands-on, "merge?".

**Deviations found while building PR3** (the sections above are updated where they apply):

- Cartesia's voice is sent as `{"id": ...}`, the shape of the researched `2026-08-14` version (13.1); the older `{"mode": "id", "id": ...}` is not used (review of PR3: it was briefly sent, but no live run confirmed that `2026-08-14` still accepts it). `generation_config` is left out at speed 1.0, as ElevenLabs' `voice_settings` is, so a model without it is not asked for it.
- Mapping rows the tables of 13.2 leave open: Cartesia 403 other than `plan_upgrade_required` is `auth`, a 404 without an `error_code` whose text names the voice (older versions' `Title: Message` bodies) is `bad_voice`, and a `bad_config` that mentions the version, or `model_not_found`, says to check `options.api_version` and the model. Deepgram's error bodies (`err_code`, `err_msg`) are read by the shared `ErrorBody`; a 400 naming `speed` is `bad_config` (it triggers the retry without speed), one naming the model is `bad_voice`.
- Option validation as for PR2: Cartesia `api_version` a date (digits and `-`), `language` a language code, `sample_rate` one of Cartesia's rates; Deepgram `sample_rate` one of 8000, 16000, 24000, 32000, 48000, and a `model` is refused (the voice is the model); `command` refuses `url` and `model`, `{out}` without `output: file`, more than 64 `argv` entries, and voices that are not plain text.
- `Adapter::adapt(&HttpRequest, &ExtError) -> bool` (default `false`) is how Deepgram drops `speed`: after a failed request the engine asks the adapter once per part, with the request that failed, whether it changed what it sends, and if so rebuilds and sends the request again. Deepgram decides from `ExtError::refused_param`, which its `map_error` sets only when the provider's error body names `speed` (never from the message, which holds the user's label), and only when the failed URL carried `&speed=`.
- `command` (review of PR3): the exact key is cut out of the program's error output before the usual masking (a key without a known prefix, Deepgram's or Azure's hex keys, would pass the mask); the program runs in a Job Object with kill-on-close, so a timeout, a cancel or the program's own exit also ends every process it started (a launcher such as `py.exe`, or a child left holding the output pipe).
- `command` (security review of PR3, 2026-10-04): any authenticated protocol client (a TCP token holder, the settings page in a browser over HTTP, the SDKs) could add or replace a `command` profile whose `argv` names any program, which sonarad then runs; the token is shared with browser pages, so any token exposure became code execution. Decision: **a `command` engine is configured only locally**. `engine_add` of kind `command`, or one that would replace a `command` profile (with any kind), is `E_FORBIDDEN` (new code, HTTP 403, protocol 1.3), checked first so the reply never reveals whether a path exists. The user edits `engines.json`, or `sonara engines add <id> --kind command` (run by the user) writes it directly (atomic, other entries kept) and sends the new `engine_reload` (protocol 1.3, no fields: it never takes a profile; an unchanged profile keeps its engine, a changed one is replaced, a gone one unregistered and a current one switched to the default choice); an entry the runtime lists with `error` is taken out of the file again. `engine_remove`, `engine_key`, `engine_test` and `set engine` stay allowed. A protocol `engine_add` or `engine_remove` first reloads `engines.json` when it changed on disk since the runtime last read or saved it (lost update between the CLI's write and its reload; a file that is not JSON is refused, never overwritten). Execution hardening: the program must be an absolute path to an existing `.exe` before each start (no PATH search); argv is passed entry by entry (`std::process::Command`, Windows quoting, never `cmd /c`); placeholders are filled in one pass; **the text read is never in argv** (`{text}` and `input: arg` were removed, `input: file` with `{in}`, a temporary UTF-8 file, added); a `{voice}` value must be in `options.voices` when that list is set and may not start with `-` or hold control characters; without that list it must be a plain name (letters, digits, `_`, `-`, `.`, no `..`), so a client cannot point it at a UNC share or a local file (follow-up review). Tests: `sonarad` `tests/engines.rs` (TCP and HTTP refusals, reload), `conformance/engines/test_command_engine.py` (TCP, HTTP 403, reload, literal shell characters), `conformance/plugin/test_cli.py` (the CLI path), `sonara-engine` `external_more.rs` (metacharacters literal, text on stdin or `{in}` only, voice checks).
- The `command` program must exist when the profile is **added** (`Profile::check_new`, called by `engine_add`), not when `engines.json` is loaded: a stored profile whose program is gone (a drive not mounted yet) stays registered, reads with the fallback (`bad_config`) and works again once the program is back. Only `.exe` is accepted, so `.cmd` is refused like `.bat`.
- A `command` voice is optional (no voice is not `bad_config` for this kind). Messages name the program by its file name only, never its folder. When no key resolves, `SONARA_ENGINE_KEY` is removed from the child's environment, so it never inherits one.
- `CancelToken::run` now also drops a result that arrives after a cancel (a program killed on the cancel answers at once with its failure, which must not reach the fallback); test `a_result_after_a_cancel_is_dropped`.
- The test program is `src/bin/sonara-fake-tts.rs` with `required-features = ["test-util", "external"]`; Cargo builds it for the crate's own integration tests (`env!("CARGO_BIN_EXE_sonara-fake-tts")`), so no separate crate was needed. The conformance test of `command` (`conformance/engines/test_command_engine.py`) runs the Python interpreter with a small script as the program instead, since conformance builds only `sonarad`, `sonara-hook` and `sonara-cli`.
- Live tests (`cartesia_live`, `deepgram_live`, `command_live`) were written but not run (no provider keys or local speech program on the build machine); step 6's hands-on decides them.

### PR4 (#227, 0.18.0): settings page and README

- [ ] 1. Load the design skills named in the user's rules; follow the page's existing structure.
- [ ] 2. Engines section per 11.3 in `settings.html`; `settings_page.rs` unchanged unless the CSP needs nothing new (it does not: same-origin API only).
- [ ] 3. E2E (`tests/e2e/test_sonarad_engines_e2e.py`, headless Playwright, `sonarad --engine fake --keys fake --system fake` with a Python fake OpenAI server): add a profile with a key (the key field empties after save and the key never appears in the DOM or in the page's network replies), the voice select fills from `voices`, Test speaks (reply shown), the "Sends text to" line, Use switches `engine`, Remove with confirmation, an auth failure shows its reason, keyboard-only path.
- [ ] 4. README Engines section (12); `PRIVACY.md` mentions the page.
- [ ] 5. Version 0.18.0, gates incl. e2e, hands-on in the browser on this PC, "merge?".

**Deviations found while building PR4** (the sections above are updated where they apply):

- Branch `feat/227-engines-settings` (the table said `feat/227-engines-page`).
- Engines is its own page in the side navigation, directly under Speech, rather than a group inside the Speech page: the list and the add/edit form are too long to share a page with the voice and rate. The nav entry is hidden when `hello.capabilities` lacks `engines` (`--no-external-engines`). Element ids avoid `#engine-rows` and `#engine-select`, which the #214 test keeps absent (the old picker).
- After a profile is added the form stays open, now editing it, so its voice list (`voices {engine, refresh: true}`) fills at once; Close hides it.
- Each kind's main options have their own fields (Azure region, ElevenLabs output format and language, Google language and billing project, Cartesia language and API version, OpenAI-compatible instructions, the `command` program, input, output, sample rate and voices); `timeout_ms` and `prefetch` sit under "More options" with a JSON object field for the rest, so every option of 5.4 stays reachable.
- An untried profile reports `ready` (7.2: `warm` does not check a key on a cloud kind until the first chunk), so the page shows "Needs a key before it can read" for a profile whose key ref needs a key that is not present.
- The Speech section names an external engine by its label and shows the reason of `engine_status.reason` (`OpenAI, cannot be reached. Kokoro reads meanwhile.`); Kokoro shows as "Kokoro (built in)".
- `settings_page.rs` is unchanged (same-origin API, no CSP change). `docs/protocol-v1.md`'s Settings page paragraph lists the Engines section.
- Muted means nothing is sent (8.5), added to this PR at the user's request of 2026-10-04.
- Step 5's hands-on (a branch build deployed per the safe redeploy steps) is left for the "merge?" step: the PR4 worker must not touch `%LOCALAPPDATA%\Sonara` or the running runtime. The page was checked headless (Playwright e2e and screenshots in light and dark, desktop and phone width).

### Gemini (#235, 0.19.0): kind `gemini`

Added on the user's request of 2026-10-05 (make Sonara ready for Gemini TTS), after PR4. Facts: the official ai.google.dev pages (speech generation guide, API reference, models, pricing, terms, troubleshooting), fetched 2026-10-05 (13.1, 13.2, 13.3 items 9 to 11).

- [x] 1. `profile.rs`: `Kind::Gemini` (wire `gemini`, label `Gemini`), default url, a default model, a default voice and a fixed list of prebuilt voices (all removed by "No model or voice in code" below), options `language_code`, `style`, `chunk_chars`, default `timeout_ms` 60000; test `gemini_profiles`.
- [x] 2. `gemini.rs`: request golden, base64 PCM (and WAV) decode, no audio is `server`, the mapping table of 13.2 incl. 429 with `retryDelay` (per minute against per day), 403, 400 bad voice, a refused field dropped once (`adapt`), the fixed voice list.
- [x] 3. `rate.rs` `gemini_pace` (9). `health.rs`: a long `Retry-After` opens the breaker at once; a quota block keeps a longer one.
- [x] 4. Joined chunks: `Engine::chunk_chars` (default 0), `Reader::set_chunk_chars` and `reader::join_chunks` in `sonara-core`, set by the worker at start and on `set engine`; `External::chunk_chars` is the profile's.
- [x] 5. `external_gemini.rs` against the scripted server (key only in `x-goog-api-key`, no key in the URL, muted sends nothing, fallback and cue, `retryDelay` respected, key bound to Google's origin, no redirect); `external_live.rs` `gemini_live`.
- [x] 6. `sonarad` kinds, CLI usage, conformance fake `Gemini` in `fakes.py`, settings page (provider, voices without a key, a required key, the AI Studio and free-tier hint, Style and Language fields), e2e test.
- [x] 7. Docs: protocol-v1, PRIVACY (Gemini's free tier may use the text), README, architecture, bundling, the CLAUDE.md live list. Version 0.19.0.
- [x] 8. Review fixes: one adapt per refusable field (`MAX_ADAPTS`, test `a_model_that_refuses_both_fields_loses_each_once_and_still_reads`); `quick_start` (`Engine::quick_start`, `Reader::set_quick_start`, `reader::join_whole`) so a reply under `chunk_chars` is one request; default `prefetch` 1; a per-day 429 waits for the daily reset and is never retried at once; the default timeout grows with `chunk_chars`; the neutral rate band covers the default 250 wpm; settings fields "Characters per request" and "Quick start".

**Decisions**

- **Endpoint**: `generateContent`, not the Interactions API the guide now leads with: the Interactions API stores each interaction by default (1 day free, 55 days paid) and is beta; `generateContent` is "fully supported" and stores nothing.
- **Model**: first a default model in code; superseded by "No model or voice in code" below: the user picks one from `GET /v1beta/models`.
- **Audio**: `responseFormat.audio` `AUDIO_L16` at 24 kHz, decoded as raw PCM without magic-number sniffing (`raw_pcm`); a WAV answer is read too.
- **Rate**: Gemini has no speed parameter. The rate becomes a style in `speechMetadata` (documented for sustained delivery, e.g. "speaking slowly"), in four bands, never in the text (3.8 reads the text verbatim, so an inline direction would be spoken). Approximate by nature; the README says so.
- **Requests**: superseded by "Send to the engine" below (#235): the joined chunks of `chunk_chars`/`quick_start` never saw more than one sentence per item, because the agent speaks each sentence as its own entry; a whole message is now one request.
- **429**: `quota` only for a daily or spend limit (the quota id), else `rate_limited`; both keep the `retryDelay` (`health.rs`), so nothing is sent before it ends and the fallback reads with one cue. A per-day quota waits for the daily reset (08:00 UTC, at least an hour) instead of a short `retryDelay`, and a `quota` is never retried at once.
- **Voices**: superseded by "No model or voice in code" below: `GET /v1beta/voices`.
- **Live**: not run (no key on the build machine); 13.3 item 9 lists what `gemini_live` decides at the hands-on step.

### No model or voice in code (#235, user decision of 2026-10-05)

The live test showed two things: a default voice that was not the one the user picked (the CLI test used the profile's voice while the user had picked another as the voice setting), and Gemini's `generateContent` taking 43 s for 2 s of audio (a preview hung 60 s). The user's decisions, for every kind and preset:

1. **No model id and no voice in code**: no default model, no default voice, no fixed voice list (removed: `GEMINI_MODEL`, `GEMINI_VOICE`, `GEMINI_VOICES`, `ELEVENLABS_MODEL`, `CARTESIA_MODEL`, the presets' default models and voices, `OPENAI_VOICES`, `OPENEDAI_VOICES`, the page's `VOICE_EXAMPLES` and placeholders). Examples in docs and CLI help use `<model>` and `<voice>`.
2. **Models**: fetched live where the provider lists them (`Adapter::models`, `parse_models`, `next_models_page`; `External::refresh_models` with the voice list's cache rules: 10 minutes, a failure kept for a minute, nothing while muted): Gemini `GET /v1beta/models` (ids naming `tts`), OpenAI-compatible `GET {url}/models` (OpenAI: ids naming `tts`; a server: those marked text-to-speech, else all; a failure is empty), ElevenLabs `GET /v1/models` (`can_do_text_to_speech`; a failure is empty: a key may lack the permission). Cartesia: typed, with a link. Required (red star) where the API needs a model in each request (Gemini, Cartesia, OpenAI, LocalAI, Speaches), optional and left out of the request elsewhere (ElevenLabs, the other presets). A refused model maps to `bad_config` naming it (`error::model_message`). Protocol 1.5 `engine_models` (saved or draft profile).
3. **Voices**: live where a voice API exists (Gemini `GET /v1beta/voices`, verified against the API reference of 2026-10-05: stored voices then the prebuilt catalog, paged), required everywhere, never preselected; where none exists (OpenAI, openedai-speech) the voice is typed and the hint links the provider's voice page.
4. **The voice rule**: the request's voice, else the reader's voice setting while the engine is current, else the profile's; with none, "choose a voice" (`bad_config`, no block: the next pick applies at once). `engine_test` follows it (`engines_ext::test`), as reading and `preview` already did.
5. **Gemini latency**: `streamGenerateContent?alt=sse` (the generateContent family: stores nothing, unlike the Interactions API's stream; the TTS guide shows streaming only with Interactions, so a refusal falls back to `generateContent` once, `adapt`). `sse.rs` reads events as they come; `streaming.rs` returns from `synthesize` at the first audio within `first_audio_ms` (default 12 s; past it a `timeout` failure: fallback with the cue, the breaker counts it); the rest follows on the `PcmStream`, bounded by a stall limit (`first_audio_ms` between pieces) and `answer_ms` (superseded by item 10 of "Send to the engine"); an answer cut after audio keeps that audio (a notice says why); a sample that one event cuts in half is joined to the next event (`stream_piece` carry). Open before merge: whether `streamGenerateContent` sends TTS audio in several events (not one at the end) is not yet checked live; run the opt-in `external_live` Gemini test with a key and note here how many audio events came and when the first one did. Known limits: after a first-audio timeout the request thread stays open until its next event, or until `first_audio_ms` when the headers had not come, or `answer_ms` (closing the socket at once needs a custom ureq transport), so a provider that already started may still bill that request; in send mode `sentence` a cut after audio does not read the rest with the fallback (13.2). The reader plays a streamed chunk from its first piece: `Engine::streams`, `Done::Part`, `Output::play_open`/`append`/`finish` (rodio, null and test outputs), the whole audio kept for a replay.
6. **Migration**: validation no longer requires a model or voice, so a stored profile keeps loading; what it stored is used as is (the user's ElevenLabs voice and model); a profile that stored nothing it needs reports `missing` in the view and `bad_config` "Choose a model ..." / "Choose a voice ..." in its status, and the settings page row says "Choose a model and a voice (Edit)".
7. Docs: protocol-v1 (1.5, the voice rule, `engine_models`, the placeholders), README, PRIVACY (model lists, the stream call), architecture, bundling, CLI help, this spec.

- [x] Tests first: `profile.rs` (no defaults, `missing_model`, `first_audio_ms`), `gemini.rs` (voices and models lists, stream request and events, a refused stream, model errors), `openai.rs`/`elevenlabs.rs`/`cartesia.rs` (no model sent when none, model lists, model errors, `instructions` adapt), `sse.rs` (events one by one, a refusal, no server), `external_gemini.rs` (playback from the first event, a late first audio falls back within the limit, a cut stream keeps its audio, a refused stream reads whole answers, a retired model is named, choose a model/voice), `external_openai.rs`, sonarad (`engine_test_uses_the_voice_picked_in_sonara`, `engine_models_come_live_for_a_saved_or_an_unsaved_profile`, `stored_profiles_without_a_model_or_voice_say_choose_one`), reader (`a_streamed_chunk_plays_as_its_audio_arrives`, `a_streamed_chunk_replays_whole`), null output (open chunks), conformance (Gemini streamed with live lists, `engine_models`, the voice rule), SDKs (`models`), e2e (Gemini's live model and voice lists with required stars and nothing preselected, OpenAI's model list and typed voice, a stored profile that says choose).

### Send to the engine (#235, user decision of 2026-10-05)

Evidence (live, 2026-10-04 18:04): with Gemini current and `read_mode` `done`, one reply of 18 sentences became 18 channel entries, 18 reader items and 18 Gemini requests; the free tier ran out (429) and each request waited 11 to 43 s. The cause was above the reader: L3 (`rules.rs`) speaks each finished sentence as its own `Speak`, so the reader's joined chunks (`chunk_chars`, `quick_start`) never saw more than one sentence per item.

Decision: every engine profile gets "Send to the engine" (`send_mode`), chosen in the engine form.

- `message` ("Full message in one request"), the default of every cloud kind (and of an `openai-compatible` server off this PC): what the agent releases for speech at once is joined into one request (paragraph breaks kept as a blank line), split only past the provider's input limit, and played as its audio streams in.
- `sentence` ("As it comes in"), the default of `command` and of an `openai-compatible` server at a loopback address: each sentence its own request, as before. Local servers stay per sentence because nothing is billed and a local server makes a whole answer before it sends it (latency: one sentence, not one reply). A cloud kind behind a local proxy keeps `message` (as it keeps its key).
- Kokoro and OneCore are not profiles: always per sentence, no setting.

How it is built:

1. **Engine** (`sonara-engine`): `SendMode` and `InputLimit` (moved from `split::Limit`, which aliases it); `Engine::send_mode` (default `Sentence`) and `Engine::input_limit`, replacing `chunk_chars` and `quick_start`. `Profile.send_mode` (top level, stored only when set; `default_send_mode`); `chunk_chars` (200..=5000) and `first_audio_ms` become common options; the pre-release `quick_start` is dropped when read and `chunk_chars: 0` reads as `send_mode: "sentence"`. `External::input_limit` is the adapter's limit or the profile's `chunk_chars` when lower; `External::streams` is true in `message` mode.
2. **Reader** (`sonara-core`, `sonara-reader`): `Chunking::{Sentences, Message{max, bytes}}` and `Reader::set_chunking` replace `set_chunk_chars`/`set_quick_start` (`join_chunks`, `join_whole`, `JOIN_MIN` removed). `pack_message`: one chunk per item, paragraphs joined with a blank line; past the limit cut at paragraph boundaries, inside a paragraph that does not fit at sentences, as few chunks as possible (when paragraph cuts would need more chunks than sentence cuts, the fewest win); a sentence longer than the limit stays whole (the engine splits it at words). The worker sets it at start and on `set engine` (a replaced profile included) and publishes it in `ReaderHandle::send_mode` (an atomic, no round trip).
3. **Agent** (`sonara-agent`): the driver calls `Rules::set_whole_messages` from `ReaderHandle::send_mode` before every rules call (messages, timers, summaries, flush). With it, `flush_prose` joins the held prose into one `Speak` (one L2 entry, one item). Release points: `done` the turn end; `queue` the batch at `minqueue`, a tool run; `immediate` the end of a paragraph (a blank line, or a block's `final`), so speech starts after the first paragraph and each paragraph is one request (a sentence-per-request `immediate` would defeat the setting); the prose before a decision. **Decisions** stay their own short `Speak` (read with priority via `prioritize`; joining would delay them behind the prose and break the priority rule): one extra small request per decision. Tool announcements stay their own too. A short focused turn-end in summary mode is joined with spaces. Mute, flush, answer and turn-start rules are unchanged (they drop the held prose before any join).
4. **Streaming in `message` mode**: Gemini's SSE as before; a raw PCM body read as it arrives (`Adapter::bytes_request`, `sse::start_bytes`): ElevenLabs `/v1/text-to-speech/{voice}/stream` with a `pcm_*` format, OpenAI-compatible with `response_format: "pcm"` (the chunked answer), Cartesia `/tts/bytes`; a body whose `Content-Type` names WAV, compressed audio or text is read whole (`Event::Whole`). Azure, Google, Deepgram and WAV answers are waited for whole. The first audio decides (`first_audio_ms`), as for Gemini. Raw streaming is used only in `message` mode, so `sentence` mode keeps the request shapes it had.
5. **Cancel**: a skip, flush, new turn, stop or mute ends the item; the reader cancels the engine (`drop_item`); a stream's `Rest` ends with `Cancelled` and drops its receiver, and the request thread closes the connection at the next data it reads (`ureq` cannot abort a read that waits, so an answer that sends nothing more stays open until its timeout). A whole answer in flight is abandoned (the provider may still finish and bill it). Nothing further is requested for the item. A mute (the hold) keeps what was spoken (as before).
6. **Replay**: Restart while the item plays replays its chunks from the worker's audio (no request). Up after the item ended reads it again as a new item; `CueCache::keep(.., message)` keeps the last 4 complete whole messages (at most 15M samples, about 30 MB, ten minutes of 24 kHz speech), so that replay sends no request. A cut answer is not kept.
7. **Fallback**: a failure before any audio reads the whole text with the fallback, the cue first once per episode, one notice (one `fallback` log line) per request; the fallback's audio is lazy (`fallback` chains the cue and Kokoro's stream instead of collecting), so a long message plays from its first sentence. A cut after audio keeps that audio and logs once; the rest is read with the fallback (see "Review fixes" below), and the message's following requests go on.
8. **Logs**: the `agent ... speak` line and the `read text` line hold the joined text once (one entry, one item; `read text ... chunks=1/1`).
9. **Protocol, CLI, SDKs, page**: `send_mode` in the profile and the view (`send_mode` in force, `explicit.send_mode`), protocol 1.5 (same unreleased 0.19.0); `sonara engines add --send-mode message|sentence`, `engines list` says "a whole message per request" or "a sentence per request"; TS `SendMode` type and `EngineProfile.send_mode`, Python `SEND_MODES`; the settings form's "Send to the engine" radio group (Full message in one request | As it comes in, a one-line hint each, the provider's default preselected and said to be the default, sent only once picked); the Gemini-only "Characters per request" and "Quick start" fields are removed.

10. **Review fixes (review of PR #236)**:
    - A tool run in `immediate` or `queue` releases the held prose but no longer lifts the paragraph or batch rule for the rest of the turn (`released` is set by a tool only in sentence mode); before, every later delta was its own request. The paragraph mark follows the hook's numbering: `index` numbers the deltas of a block and restarts at 0 for a new block, so only a restart (or a block's `final`, or a blank line) starts a paragraph; before, every delta was joined with a blank line.
    - Timeouts: `timeout_ms` is the wait for an answer (default 15000 cloud, 60000 loopback and Gemini, 30000 command, in both modes). A request in `message` mode may take `Profile::answer_ms` = `timeout_ms` + 70 ms per character (`ANSWER_MS_PER_CHAR`, about speaking time), set per request (`adapter::Within`: the whole answer's response and body limits). A streamed answer's headers must come within `first_audio_ms` (so a silent provider's connection is dropped then), its body within `answer_ms`, and after the first audio `first_audio_ms` is a stall limit between pieces; the length of a steady stream is never the limit. So a whole-answer provider that stalls on a short message falls back after about `timeout_ms`, not two minutes.
    - A stall, a break or `answer_ms` after audio in `message` mode reads the rest with the fallback (the cue once per episode, one notice naming the fallback) from the start of the sentence reached: `streaming::rest_of` estimates the place from the seconds played at `CHARS_PER_SECOND` 12 (slower than most voices, so the estimate lags: a sentence may be read twice, none dropped). A mute (the hold) mid-message reads the rest locally the same way, with no cue and no notice. In `sentence` mode a cut chunk ends with what was spoken, as before.
    - Costs: an abandoned whole answer and a streamed answer that started after `first_audio_ms` may still be billed once (protocol, README).
    - Settings page: "Wait for audio" (`first_audio_ms`) and "Characters per request" (`chunk_chars`) are common fields under More options, shown in `message` mode (the wait also for Gemini in `sentence`); the timeout hint speaks of answers and a full message's extra time.
    - The message cache holds at most 15M samples (was 32M).

- [x] Tests first: `reader_message.rs` (one chunk per item, paragraphs kept, paragraph-first splitting, fewest chunks, a long sentence whole, byte limits, restart without a new synthesis, queued items keep their chunks); reader `lookahead.rs` (`an_engine_in_message_mode_gets_the_whole_item`, `a_message_past_the_engines_limit_is_split_at_paragraphs`, `ReaderHandle::send_mode` follows the engine); agent `send_mode.rs` (done = one `Speak` of an 18-sentence reply, sentence mode 18; blocks are paragraphs; immediate per paragraph; queue batch; the decision after one joined `Speak`; turn start and flush drop it; muted noted) and `agent.rs` (`a_done_reply_is_one_synthesis_with_an_engine_that_takes_whole_messages`: 1 synthesis vs 18, Up replays the whole message); `profile.rs` (`send_mode_defaults_per_kind_and_round_trips`, folded pre-release options); `cache.rs`; `sse.rs` (raw bytes as they come, a WAV body whole, a dropped receiver closes the connection); `external_message.rs` (one request playing from its first piece, a replay without a request, a cancel mid-stream closes the connection and asks for nothing more, a failure reads the whole message with one cue and one notice, no first audio in time, a cut keeps what came and is not replayed, a WAV answer, OpenAI `pcm` and Cartesia); sonarad (`send_mode_is_stored_only_when_chosen`, the view, `read_text_log` one line per message); CLI (`--send-mode`, list lines); conformance `test_send_mode.py` (the view, an 18-sentence done reply is 1 request in `message` and 18 in `sentence`, a stream cut by flush, skip, turn start and mute: the fake provider sees the connection closed and no further request); SDKs (the view's `send_mode`); e2e (`test_send_to_the_engine_is_preselected_per_kind_and_saved_when_chosen`).

### Risks

| Risk | Mitigation |
|---|---|
| A cloud chunk takes longer than the sentence before it plays | lookahead 2; breaker skips a slow provider after two failures; the timeout bounds the wait |
| Keys leak into logs through provider error bodies | clip + `secrets::mask`; tests assert no key in `sonarad.log`, `hook.log`, `engines.json`, `config.json`, replies |
| `EngineId::intern` leaks memory | bounded by validation (16 profiles, 32-byte ids) per distinct id |
| The provider changes an API (dated Cartesia version, model ids, voices) | no model or voice in code (#235): live lists, typed ids; `api_version` an option; `bad_config` messages name the model |
| A `command` profile runs a program | never added or changed over the protocol (`E_FORBIDDEN`): only the user's own `engines.json`, written by hand or by `sonara engines add --kind command`, then `engine_reload`; absolute path to an existing `.exe` required before each start; no shell; the text never in argv; argv logged as the file name only |
