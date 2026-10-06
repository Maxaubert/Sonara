//! Engine profiles (spec section 5): what the user added, as stored in
//! `engines.json` and sent in `engine_add`. Parsing validates every field;
//! the JSON in and out is a `serde_json::Value` (no serde derive), so the
//! stored shape is exactly what `to_json` writes.
//!
//! `kind.rs` holds the kinds, presets and key sources, `url.rs` the
//! addresses and origins, `validate.rs` the rules of spec 5.2 and 5.4.
//! A provider's own constants live in its file and are re-exported here.
mod kind;
#[cfg(test)]
mod tests;
mod url;
mod validate;

pub use super::azure::AZURE_FORMATS;
pub use super::cartesia::{CARTESIA_RATES, CARTESIA_VERSION};
pub use super::command::{COMMAND_MAX_ARGS, COMMAND_MAX_VOICES};
pub use super::deepgram::DEEPGRAM_RATES;
pub use super::elevenlabs::ELEVENLABS_FORMATS;
pub use super::gemini::GEMINI_CHUNK_CHARS;
use crate::SendMode;
pub use kind::{implemented_kinds, KeyRef, Kind, Preset};
use serde_json::{json, Map, Value};
pub use url::{command_origin, default_base, is_loopback_host, origin_of, Url};
use validate::opt_text;
pub use validate::validate_id;

/// At most this many profiles (spec 5.2).
pub const MAX_PROFILES: usize = 16;
/// Ids of the built-in engines, refused as profile ids.
pub const BUILTIN_IDS: &[&str] = &["kokoro", "onecore", "fake"];

/// The range of `chunk_chars` (every kind, send mode `message`): the most
/// characters one request takes; the provider's own input limit still
/// applies when it is lower (`External::input_limit`).
pub const CHUNK_CHARS_MIN: u64 = 200;
pub const CHUNK_CHARS_MAX: u64 = 5000;

/// The wait for audio of a streamed answer (Gemini's events, a cloud
/// answer read as it arrives in send mode `message`), the first and each
/// next: no first audio in time reads the text with the fallback, a stall
/// after audio reads the rest with it (#235, option `first_audio_ms`).
pub const GEMINI_FIRST_AUDIO_MS: u64 = 12_000;
/// What a whole message adds to `timeout_ms` per character of its text in
/// send mode `message` (#235): about the time it takes to speak it (15
/// characters a second), so a provider that makes audio no faster than it
/// plays still finishes, while a short message keeps a short limit.
pub const ANSWER_MS_PER_CHAR: u64 = 70;

/// The file name of a program path (`C:\Tools\tts.exe` gives `tts.exe`).
pub fn program_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// The locale a voice name starts with: `en-US-<name>` gives `en-US`; a
/// name without one gives `None`.
pub fn voice_locale(voice: &str) -> Option<String> {
    let mut parts = voice.splitn(3, '-');
    let (lang, region, rest) = (parts.next()?, parts.next()?, parts.next()?);
    let ok = (2..=3).contains(&lang.len())
        && lang.chars().all(|c| c.is_ascii_lowercase())
        && (2..=4).contains(&region.len())
        && region.chars().all(|c| c.is_ascii_alphanumeric())
        && !rest.is_empty();
    ok.then(|| format!("{lang}-{region}"))
}

/// Why a profile cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// A kind this build does not implement (yet): kept, listed, not
    /// registered.
    Unsupported { id: String, kind: String },
    /// A field breaks a rule of spec 5.2 (an `E_BAD_REQUEST` message).
    Invalid(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::Unsupported { kind, .. } => {
                write!(f, "kind '{kind}' is not supported by this version")
            }
            ProfileError::Invalid(m) => f.write_str(m),
        }
    }
}

fn invalid(m: impl Into<String>) -> ProfileError {
    ProfileError::Invalid(m.into())
}

/// One engine the user added (spec 5.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub id: String,
    pub kind: Kind,
    pub label: Option<String>,
    pub url: Option<String>,
    pub model: Option<String>,
    pub voice: Option<String>,
    pub key_ref: KeyRef,
    pub options: Map<String, Value>,
    /// "Send to the engine" (#235): `None` takes the kind's default
    /// (`Profile::send_mode`).
    pub send_mode: Option<SendMode>,
    /// The origin an `env:` key may also go to besides the provider's
    /// default (spec 6.4): set by the runtime from `engines.json`
    /// (`key_origin`, written only by the user or the format 1 migration),
    /// never parsed from a protocol message, never in `to_json`.
    pub key_origin: Option<String>,
}

impl Profile {
    /// Parse and validate a profile object (`engines.json` entry or the
    /// `engine` of `engine_add`). Unknown top-level fields are ignored, so
    /// a secret sent there is never kept; unknown options are refused.
    pub fn from_json(v: &Value) -> Result<Profile, ProfileError> {
        let m = v
            .as_object()
            .ok_or_else(|| invalid("an engine profile is a JSON object"))?;
        let id = opt_text(m, "id")?.ok_or_else(|| invalid("missing 'id'"))?;
        validate_id(&id).map_err(ProfileError::Invalid)?;
        let kind_name = opt_text(m, "kind")?.ok_or_else(|| invalid("missing 'kind'"))?;
        let kind = match Kind::parse(&kind_name) {
            Some(k) if k.implemented() => k,
            _ => {
                return Err(ProfileError::Unsupported {
                    id,
                    kind: kind_name,
                })
            }
        };
        let mut options = match m.get("options") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(o)) => o.clone(),
            Some(_) => return Err(invalid("'options' must be an object")),
        };
        let mut send_mode = match m.get("send_mode") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_str()
                    .and_then(SendMode::parse)
                    .ok_or_else(|| invalid("'send_mode' must be \"message\" or \"sentence\""))?,
            ),
        };
        // The 0.19 pre-release Gemini options folded into `send_mode`
        // (#235): `quick_start` is gone, and `chunk_chars` 0 was one
        // sentence per request.
        options.remove("quick_start");
        if options.get("chunk_chars").and_then(Value::as_u64) == Some(0) {
            options.remove("chunk_chars");
            send_mode = send_mode.or(Some(SendMode::Sentence));
        }
        let mut p = Profile {
            id,
            kind,
            label: opt_text(m, "label")?,
            url: opt_text(m, "url")?,
            model: opt_text(m, "model")?,
            voice: opt_text(m, "voice")?,
            key_ref: KeyRef::None,
            options,
            send_mode,
            key_origin: None,
        };
        p.key_ref = match KeyRef::parse(m.get("key_ref")).map_err(ProfileError::Invalid)? {
            Some(k) => k,
            None => p.default_key_ref(),
        };
        p.validate()?;
        Ok(p)
    }

    /// `none` for a local OpenAI-compatible server and a program,
    /// `credman` otherwise (spec 5.2): a cloud kind always needs a key, even
    /// when its url points at a local proxy.
    pub fn default_key_ref(&self) -> KeyRef {
        let local_server = self.kind == Kind::OpenAiCompatible
            && self.parsed_url().is_some_and(|u| u.is_loopback());
        if self.kind == Kind::Command || local_server {
            KeyRef::None
        } else {
            KeyRef::CredMan
        }
    }

    /// The stored and wire form (never a secret).
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("id".into(), json!(self.id));
        m.insert("kind".into(), json!(self.kind.as_str()));
        for (k, v) in [
            ("label", &self.label),
            ("url", &self.url),
            ("model", &self.model),
            ("voice", &self.voice),
        ] {
            if let Some(v) = v {
                m.insert(k.into(), json!(v));
            }
        }
        m.insert("key_ref".into(), json!(self.key_ref.as_wire()));
        if let Some(mode) = self.send_mode {
            m.insert("send_mode".into(), json!(mode.as_str()));
        }
        m.insert("options".into(), Value::Object(self.options.clone()));
        Value::Object(m)
    }

    /// `options.argv` of a `command` profile (empty for other kinds).
    pub fn command_argv(&self) -> Vec<String> {
        if self.kind != Kind::Command {
            return Vec::new();
        }
        self.options
            .get("argv")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The program of a `command` profile (`argv[0]`).
    pub fn command_program(&self) -> Option<&str> {
        if self.kind != Kind::Command {
            return None;
        }
        self.options
            .get("argv")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
    }

    pub fn option_str(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(Value::as_str)
    }

    pub fn option_u64(&self, key: &str) -> Option<u64> {
        self.options.get(key).and_then(Value::as_u64)
    }

    pub fn allow_http(&self) -> bool {
        self.options
            .get("allow_http")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// The `openai-compatible` preset (`generic` when unset).
    pub fn preset(&self) -> Preset {
        self.option_str("preset")
            .and_then(Preset::parse)
            .unwrap_or(Preset::Generic)
    }

    /// The base URL in force: the profile's, else the kind's default.
    pub fn base_url(&self) -> Option<String> {
        self.url
            .clone()
            .or_else(|| default_base(self.kind, &self.options))
            .map(|u| u.trim_end_matches('/').to_string())
    }

    pub fn parsed_url(&self) -> Option<Url> {
        self.base_url().and_then(|u| Url::parse(&u).ok())
    }

    /// The origin requests go to (`scheme://host:port`), what a stored key
    /// must be bound to (spec 6.4).
    /// For a `command`, its program (`command_origin`).
    pub fn origin(&self) -> Option<String> {
        if self.kind == Kind::Command {
            return self.command_program().map(command_origin);
        }
        self.parsed_url().map(|u| u.origin())
    }

    /// The origin of the provider itself (the kind's or preset's default
    /// address, never the profile's `url`): an `env:` key may always go
    /// there.
    /// A `command`'s is its program: it comes only from the local file,
    /// never over the protocol.
    pub fn default_origin(&self) -> Option<String> {
        if self.kind == Kind::Command {
            return self.origin();
        }
        default_base(self.kind, &self.options)
            .and_then(|u| Url::parse(u.trim_end_matches('/')).ok())
            .map(|u| u.origin())
    }

    /// Whether the kind has a model field at all (Azure, Google, Deepgram
    /// and a program take none).
    pub fn takes_model(&self) -> bool {
        matches!(
            self.kind,
            Kind::OpenAiCompatible | Kind::ElevenLabs | Kind::Gemini | Kind::Cartesia
        )
    }

    /// Whether the provider needs a model in every request (#235): Gemini
    /// (the model is in the URL), Cartesia, and the servers of
    /// `Preset::model_required`. ElevenLabs and the other servers pick
    /// their own when Sonara sends none, so there it is optional.
    pub fn model_required(&self) -> bool {
        match self.kind {
            Kind::OpenAiCompatible => self.preset().model_required(),
            Kind::Gemini | Kind::Cartesia => true,
            _ => false,
        }
    }

    /// A model is needed and none is set: the engine says "choose a model"
    /// (`bad_config`) and reads with the fallback until one is set.
    pub fn missing_model(&self) -> bool {
        self.model_required() && self.model.is_none()
    }

    /// Whether a request needs a voice (every kind but a program, whose
    /// `{voice}` may be unused). Sonara never picks one for the user: the
    /// voice comes from the profile or the user's voice setting
    /// (`External::voice_for`).
    pub fn voice_required(&self) -> bool {
        self.kind != Kind::Command
    }

    /// The name spoken in cues and shown in lists.
    pub fn display_label(&self) -> String {
        if let Some(l) = &self.label {
            return l.clone();
        }
        match self.kind {
            Kind::OpenAiCompatible => self.preset().display_name().to_string(),
            k => k.display_name().to_string(),
        }
    }

    /// The host the text goes to (`""` for a command).
    pub fn host(&self) -> String {
        self.parsed_url().map(|u| u.host).unwrap_or_default()
    }

    /// The text stays on this PC (a loopback server or a program).
    pub fn is_local(&self) -> bool {
        self.kind == Kind::Command || self.parsed_url().is_some_and(|u| u.is_loopback())
    }

    /// `sends_text_to` of the profile view (spec 10.1): the host, or for a
    /// command `program <file name>` (never its folder or arguments).
    pub fn sends_text_to(&self) -> String {
        match self.command_program() {
            Some(p) => format!("program {}", program_name(p)),
            None => self.host(),
        }
    }

    /// Request timeout (spec 5.4 common options): the wait for an answer
    /// to a sentence. A whole message (send mode `message`) gets this plus
    /// `ANSWER_MS_PER_CHAR` per character (`answer_ms`, #235); a streamed
    /// answer is cut by a stall (`first_audio_ms` without audio), not by
    /// its length.
    pub fn timeout_ms(&self) -> u64 {
        self.option_u64("timeout_ms").unwrap_or(match self.kind {
            Kind::Command => 30_000,
            _ if self.is_local() || self.kind == Kind::Gemini => 60_000,
            _ => 15_000,
        })
    }

    /// The longest a request for `chars` characters may take, its whole
    /// answer included: `timeout_ms`, plus `ANSWER_MS_PER_CHAR` per
    /// character in send mode `message` (#235).
    pub fn answer_ms(&self, chars: usize) -> u64 {
        let per_char = match self.send_mode() {
            SendMode::Message => ANSWER_MS_PER_CHAR,
            SendMode::Sentence => 0,
        };
        self.timeout_ms() + per_char * chars as u64
    }

    /// "Send to the engine" (#235): the profile's `send_mode`, else the
    /// kind's default (`default_send_mode`).
    pub fn send_mode(&self) -> SendMode {
        self.send_mode.unwrap_or_else(|| self.default_send_mode())
    }

    /// The default send mode: `sentence` for a program and an
    /// OpenAI-compatible server on this PC (nothing is billed, and a local
    /// server would make a whole message before its first audio, while a
    /// sentence comes back at once), `message` for every cloud kind (even
    /// behind a local proxy, as for its key) and a server on another
    /// computer.
    pub fn default_send_mode(&self) -> SendMode {
        if self.kind == Kind::Command || self.default_key_ref() == KeyRef::None {
            SendMode::Sentence
        } else {
            SendMode::Message
        }
    }

    /// The most characters one request takes in send mode `message`
    /// (`options.chunk_chars`; Gemini's default `GEMINI_CHUNK_CHARS`).
    /// `None` leaves the provider's input limit alone.
    pub fn chunk_chars(&self) -> Option<usize> {
        self.option_u64("chunk_chars")
            .or((self.kind == Kind::Gemini).then_some(GEMINI_CHUNK_CHARS))
            .map(|n| n as usize)
    }

    /// How long a streamed answer may go without audio (#235): before its
    /// first audio the text is read with the fallback, after it the rest
    /// is. `options.first_audio_ms`, default `GEMINI_FIRST_AUDIO_MS`, never
    /// more than `timeout_ms`.
    pub fn first_audio_ms(&self) -> u64 {
        self.option_u64("first_audio_ms")
            .unwrap_or(GEMINI_FIRST_AUDIO_MS)
            .min(self.timeout_ms())
    }

    /// Chunks synthesized ahead (spec D10).
    pub fn prefetch(&self) -> usize {
        self.option_u64("prefetch")
            .map(|n| n.clamp(1, 4) as usize)
            .unwrap_or(if self.is_local() || self.kind == Kind::Gemini {
                // Gemini: one ahead, so the start of a reply is not a burst
                // against the free tier's per-minute limit (#235).
                1
            } else {
                2
            })
    }

    /// Whether the kind needs a key to work at all (a key_ref other than
    /// `none`).
    pub fn needs_key(&self) -> bool {
        self.key_ref != KeyRef::None
    }
}
