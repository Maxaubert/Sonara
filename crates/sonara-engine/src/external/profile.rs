//! Engine profiles (spec section 5): what the user added, as stored in
//! `engines.json` and sent in `engine_add`. Parsing validates every field;
//! the JSON in and out is a `serde_json::Value` (no serde derive), so the
//! stored shape is exactly what `to_json` writes.
use serde_json::{json, Map, Value};

/// At most this many profiles (spec 5.2).
pub const MAX_PROFILES: usize = 16;
/// Ids of the built-in engines, refused as profile ids.
pub const BUILTIN_IDS: &[&str] = &["kokoro", "onecore", "fake"];

/// The provider shape of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    OpenAiCompatible,
    ElevenLabs,
    Azure,
    Google,
    Cartesia,
    Deepgram,
    Command,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::OpenAiCompatible,
        Kind::ElevenLabs,
        Kind::Azure,
        Kind::Google,
        Kind::Cartesia,
        Kind::Deepgram,
        Kind::Command,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::OpenAiCompatible => "openai-compatible",
            Kind::ElevenLabs => "elevenlabs",
            Kind::Azure => "azure",
            Kind::Google => "google",
            Kind::Cartesia => "cartesia",
            Kind::Deepgram => "deepgram",
            Kind::Command => "command",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Whether this build implements the kind (a profile of another kind is
    /// kept in `engines.json` and listed, not registered).
    pub fn implemented(&self) -> bool {
        matches!(self, Kind::OpenAiCompatible)
    }
}

/// The kinds this build implements, in wire order (`engine_list.kinds`).
pub fn implemented_kinds() -> Vec<Kind> {
    Kind::ALL.into_iter().filter(Kind::implemented).collect()
}

/// A known server behind the `openai-compatible` kind (spec 5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Preset {
    OpenAi,
    KokoroFastApi,
    LocalAi,
    Speaches,
    OpenedAiSpeech,
    ChatterboxApi,
    ChatterboxServer,
    Generic,
}

impl Preset {
    pub const ALL: [Preset; 8] = [
        Preset::OpenAi,
        Preset::KokoroFastApi,
        Preset::LocalAi,
        Preset::Speaches,
        Preset::OpenedAiSpeech,
        Preset::ChatterboxApi,
        Preset::ChatterboxServer,
        Preset::Generic,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Preset::OpenAi => "openai",
            Preset::KokoroFastApi => "kokoro-fastapi",
            Preset::LocalAi => "localai",
            Preset::Speaches => "speaches",
            Preset::OpenedAiSpeech => "openedai-speech",
            Preset::ChatterboxApi => "chatterbox-api",
            Preset::ChatterboxServer => "chatterbox-server",
            Preset::Generic => "generic",
        }
    }

    pub fn parse(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.as_str() == s)
    }

    /// Spoken in cues and shown when the profile has no label.
    pub fn display_name(&self) -> &'static str {
        match self {
            Preset::OpenAi => "OpenAI",
            Preset::KokoroFastApi => "Kokoro FastAPI",
            Preset::LocalAi => "LocalAI",
            Preset::Speaches => "Speaches",
            Preset::OpenedAiSpeech => "openedai-speech",
            Preset::ChatterboxApi => "Chatterbox",
            Preset::ChatterboxServer => "Chatterbox server",
            Preset::Generic => "The speech server",
        }
    }

    pub fn default_model(&self) -> Option<&'static str> {
        match self {
            Preset::OpenAi => Some("gpt-4o-mini-tts"),
            Preset::KokoroFastApi => Some("kokoro"),
            Preset::LocalAi | Preset::Speaches => None,
            Preset::OpenedAiSpeech | Preset::Generic => Some("tts-1"),
            Preset::ChatterboxApi | Preset::ChatterboxServer => Some("chatterbox"),
        }
    }

    pub fn default_voice(&self) -> Option<&'static str> {
        match self {
            Preset::OpenAi => Some("marin"),
            Preset::KokoroFastApi | Preset::Speaches => Some("af_heart"),
            Preset::LocalAi | Preset::ChatterboxServer => None,
            Preset::OpenedAiSpeech | Preset::ChatterboxApi | Preset::Generic => Some("alloy"),
        }
    }

    /// The URL used when the profile names none (only OpenAI has one).
    pub fn default_url(&self) -> Option<&'static str> {
        match self {
            Preset::OpenAi => Some("https://api.openai.com/v1"),
            _ => None,
        }
    }
}

/// Where a profile's key comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyRef {
    /// No key (a local server).
    None,
    /// Windows Credential Manager, target `sonara:<profile id>`.
    CredMan,
    /// The environment variable of this name (of the runtime's process).
    Env(String),
}

impl KeyRef {
    /// The wire form: `"none"`, `"credman"`, `"env:NAME"`.
    pub fn as_wire(&self) -> String {
        match self {
            KeyRef::None => "none".into(),
            KeyRef::CredMan => "credman".into(),
            KeyRef::Env(n) => format!("env:{n}"),
        }
    }

    /// `null`/absent is `Ok(None)` (the kind's default applies).
    pub fn parse(v: Option<&Value>) -> Result<Option<KeyRef>, String> {
        let s = match v {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(s)) => s.as_str(),
            Some(_) => return Err("'key_ref' must be \"none\", \"credman\" or \"env:NAME\"".into()),
        };
        match s {
            "none" => Ok(Some(KeyRef::None)),
            "credman" => Ok(Some(KeyRef::CredMan)),
            _ => match s.strip_prefix("env:") {
                Some(name) if env_name_ok(name) && env_name_allowed(name) => {
                    Ok(Some(KeyRef::Env(name.to_string())))
                }
                Some(name) if env_name_ok(name) => Err(format!(
                    "environment variable '{name}' in key_ref must end in _API_KEY or _SPEECH_KEY, or start with SONARA_"
                )),
                Some(name) => Err(format!(
                    "invalid environment variable name '{name}' in key_ref"
                )),
                None => Err(format!(
                    "unknown key_ref '{s}': use \"none\", \"credman\" or \"env:NAME\""
                )),
            },
        }
    }
}

fn env_name_ok(n: &str) -> bool {
    let mut chars = n.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && n.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Only a variable named like an API key (`*_API_KEY`; Azure's
/// `SPEECH_KEY`, `*_SPEECH_KEY`), or one of Sonara's own, may be
/// sent to a provider: a protocol client must not be able to send the
/// runtime's other secrets (`GITHUB_TOKEN`, cloud credentials) to a URL it
/// chose. Windows names are case-insensitive.
fn env_name_allowed(n: &str) -> bool {
    let n = n.to_ascii_uppercase();
    let suffix = |x: &str| n.ends_with(x) && n.len() > x.len();
    suffix("_API_KEY") || suffix("_SPEECH_KEY") || n == "SPEECH_KEY" || n.starts_with("SONARA_")
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

/// The parts of an `http(s)` URL Sonara uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub https: bool,
    /// Lower case, IPv6 without brackets.
    pub host: String,
    pub port: Option<u16>,
    /// Starts with `/` or is empty.
    pub path: String,
}

impl Url {
    /// Absolute `http`/`https` only; no userinfo, query or fragment.
    pub fn parse(s: &str) -> Result<Url, String> {
        let bad = |why: &str| format!("invalid url '{s}': {why}");
        let (https, rest) = if let Some(r) = s.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = s.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(bad("use an absolute http:// or https:// address"));
        };
        if s.contains('?') || s.contains('#') {
            return Err(bad("no query or fragment"));
        }
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(bad("no user name or password in the address"));
        }
        if s.chars().any(|c| c.is_control() || c == ' ') {
            return Err(bad("no spaces or control characters"));
        }
        let (host, port) = if let Some(r) = authority.strip_prefix('[') {
            let end = r.find(']').ok_or_else(|| bad("unclosed '['"))?;
            let port = r[end + 1..].strip_prefix(':');
            (r[..end].to_string(), port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (authority.to_string(), None),
            }
        };
        if host.is_empty() {
            return Err(bad("no host"));
        }
        let port = match port {
            None => None,
            Some(p) => Some(p.parse::<u16>().map_err(|_| bad("bad port"))?),
        };
        Ok(Url {
            https,
            host: host.to_ascii_lowercase(),
            port,
            path: path.to_string(),
        })
    }

    pub fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }

    /// `scheme://host:port`, the port always written (443 or 80 when the
    /// URL has none), an IPv6 host in brackets: what a stored key is bound
    /// to (spec 6.4).
    pub fn origin(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        let port = self.port.unwrap_or(if self.https { 443 } else { 80 });
        if self.host.contains(':') {
            format!("{scheme}://[{}]:{port}", self.host)
        } else {
            format!("{scheme}://{}:{port}", self.host)
        }
    }
}

/// An Azure region as it goes into a host name (`westeurope`).
fn region_ok(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 40
        && r.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The base URL a kind uses when the profile names no `url` (spec 5.4):
/// the provider's address, Azure's from `options.region`, the
/// `openai-compatible` preset's (only `openai` has one). `None` for a kind
/// without one (`command`, a local server's preset).
pub fn default_base(kind: Kind, options: &Map<String, Value>) -> Option<String> {
    match kind {
        Kind::OpenAiCompatible => options
            .get("preset")
            .and_then(Value::as_str)
            .and_then(Preset::parse)
            .unwrap_or(Preset::Generic)
            .default_url()
            .map(str::to_string),
        Kind::ElevenLabs => Some("https://api.elevenlabs.io".into()),
        Kind::Azure => options
            .get("region")
            .and_then(Value::as_str)
            .filter(|r| region_ok(r))
            .map(|r| format!("https://{r}.tts.speech.microsoft.com")),
        Kind::Google => Some("https://texttospeech.googleapis.com".into()),
        Kind::Cartesia => Some("https://api.cartesia.ai".into()),
        Kind::Deepgram => Some("https://api.deepgram.com".into()),
        Kind::Command => None,
    }
}

/// The origin an `engines.json` entry sends to, for any kind, supported by
/// this build or not: its `url`, else the kind's default (`default_base`).
/// `None` when it has no valid address.
pub fn origin_of(raw: &Value) -> Option<String> {
    let url = raw
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let kind = Kind::parse(raw.get("kind").and_then(Value::as_str)?)?;
            let empty = Map::new();
            let options = raw
                .get("options")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            default_base(kind, options)
        })?;
    Url::parse(url.trim_end_matches('/'))
        .ok()
        .map(|u| u.origin())
}

/// `localhost`, `127.0.0.0/8` or `::1`.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    if h.eq_ignore_ascii_case("localhost") || h == "::1" {
        return true;
    }
    match h.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => h
            .parse::<std::net::Ipv6Addr>()
            .is_ok_and(|ip| ip.is_loopback()),
    }
}

/// `^[a-z0-9][a-z0-9_-]{0,31}$`, not a built-in id, not `sonara*`.
pub fn validate_id(id: &str) -> Result<(), String> {
    let ok = id.len() <= 32
        && id
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        return Err(format!(
            "invalid engine id '{id}': use 1 to 32 of a-z, 0-9, '-' and '_'"
        ));
    }
    if BUILTIN_IDS.contains(&id) {
        return Err(format!("'{id}' is a built-in engine"));
    }
    if id.starts_with("sonara") {
        return Err(format!("'{id}': ids starting with 'sonara' are reserved"));
    }
    Ok(())
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
    /// The origin an `env:` key may also go to besides the provider's
    /// default (spec 6.4): set by the runtime from `engines.json`
    /// (`key_origin`, written only by the user or the format 1 migration),
    /// never parsed from a protocol message, never in `to_json`.
    pub key_origin: Option<String>,
}

/// Options every kind accepts.
const COMMON_OPTIONS: &[&str] = &["timeout_ms", "prefetch", "allow_http"];
/// Options of `openai-compatible`.
const OPENAI_OPTIONS: &[&str] = &[
    "preset",
    "response_format",
    "sample_rate",
    "instructions",
    "extra",
    "voices_path",
];

fn opt_text(m: &Map<String, Value>, field: &str) -> Result<Option<String>, ProfileError> {
    match m.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid(format!("'{field}' must be a string"))),
    }
}

fn plain_text(field: &str, s: &str, max: usize) -> Result<(), ProfileError> {
    if s.chars().count() > max {
        return Err(invalid(format!(
            "'{field}' is longer than {max} characters"
        )));
    }
    if s.chars().any(char::is_control) {
        return Err(invalid(format!("'{field}' has control characters")));
    }
    Ok(())
}

fn int_in(
    o: &Map<String, Value>,
    field: &str,
    range: std::ops::RangeInclusive<u64>,
) -> Result<(), ProfileError> {
    match o.get(field) {
        None => Ok(()),
        Some(v) => match v.as_u64() {
            Some(n) if range.contains(&n) => Ok(()),
            _ => Err(invalid(format!(
                "option '{field}' must be a whole number in {}..={}",
                range.start(),
                range.end()
            ))),
        },
    }
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
        let options = match m.get("options") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(o)) => o.clone(),
            Some(_) => return Err(invalid("'options' must be an object")),
        };
        let mut p = Profile {
            id,
            kind,
            label: opt_text(m, "label")?,
            url: opt_text(m, "url")?,
            model: opt_text(m, "model")?,
            voice: opt_text(m, "voice")?,
            key_ref: KeyRef::None,
            options,
            key_origin: None,
        };
        p.key_ref = match KeyRef::parse(m.get("key_ref")).map_err(ProfileError::Invalid)? {
            Some(k) => k,
            None => p.default_key_ref(),
        };
        p.validate()?;
        Ok(p)
    }

    /// `none` for a local server, `credman` otherwise (spec 5.2).
    pub fn default_key_ref(&self) -> KeyRef {
        if self.kind == Kind::Command || self.parsed_url().is_some_and(|u| u.is_loopback()) {
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
        m.insert("options".into(), Value::Object(self.options.clone()));
        Value::Object(m)
    }

    /// Spec 5.2 and the kind's rules of 5.4.
    pub fn validate(&self) -> Result<(), ProfileError> {
        validate_id(&self.id).map_err(ProfileError::Invalid)?;
        if let Some(l) = &self.label {
            plain_text("label", l, 40)?;
        }
        for (f, v) in [("model", &self.model), ("voice", &self.voice)] {
            if let Some(v) = v {
                plain_text(f, v, 200)?;
            }
        }
        if let KeyRef::Env(n) = &self.key_ref {
            if !env_name_ok(n) {
                return Err(invalid(format!("invalid environment variable name '{n}'")));
            }
        }
        self.validate_options()?;
        let allow_http = self.allow_http();
        if let Some(u) = &self.url {
            let url = Url::parse(u).map_err(ProfileError::Invalid)?;
            if !url.https && !url.is_loopback() {
                if !allow_http {
                    return Err(invalid(format!(
                        "'{u}' is plain http to another computer: use https, or set \
                         options.allow_http for a server on your network"
                    )));
                }
                if self.key_ref != KeyRef::None {
                    return Err(invalid(
                        "a key is never sent over http to a non-loopback host",
                    ));
                }
            }
        }
        if self.kind == Kind::OpenAiCompatible {
            let preset = self.preset();
            if self.url.is_none() && preset.default_url().is_none() {
                return Err(invalid(format!(
                    "preset '{}' needs a url (the server's address with /v1)",
                    preset.as_str()
                )));
            }
            if self.effective_model().is_none() {
                return Err(invalid(format!(
                    "preset '{}' needs a model",
                    preset.as_str()
                )));
            }
            if self.effective_voice().is_none() && preset == Preset::ChatterboxServer {
                return Err(invalid(
                    "preset 'chatterbox-server' needs a voice (a file name such as Emily.wav)",
                ));
            }
        }
        Ok(())
    }

    fn validate_options(&self) -> Result<(), ProfileError> {
        let o = &self.options;
        let kind_opts: &[&str] = match self.kind {
            Kind::OpenAiCompatible => OPENAI_OPTIONS,
            _ => &[],
        };
        if let Some(k) = o
            .keys()
            .find(|k| !COMMON_OPTIONS.contains(&k.as_str()) && !kind_opts.contains(&k.as_str()))
        {
            return Err(invalid(format!(
                "unknown option '{k}' for kind '{}'",
                self.kind.as_str()
            )));
        }
        int_in(o, "timeout_ms", 1000..=120_000)?;
        int_in(o, "prefetch", 1..=4)?;
        if o.get("allow_http").is_some_and(|v| !v.is_boolean()) {
            return Err(invalid("option 'allow_http' must be true or false"));
        }
        if self.kind == Kind::OpenAiCompatible {
            if let Some(v) = o.get("preset") {
                let name = v.as_str().unwrap_or_default();
                if Preset::parse(name).is_none() {
                    let all: Vec<&str> = Preset::ALL.iter().map(Preset::as_str).collect();
                    return Err(invalid(format!(
                        "unknown preset '{name}' (use one of {})",
                        all.join(", ")
                    )));
                }
            }
            if let Some(v) = o.get("response_format") {
                if !matches!(v.as_str(), Some("wav") | Some("pcm")) {
                    return Err(invalid(
                        "option 'response_format' must be \"wav\" or \"pcm\"",
                    ));
                }
            }
            int_in(o, "sample_rate", 8000..=48_000)?;
            if let Some(v) = o.get("instructions") {
                let s = v
                    .as_str()
                    .ok_or_else(|| invalid("option 'instructions' must be a string"))?;
                if s.chars().count() > 4000 {
                    return Err(invalid(
                        "option 'instructions' is longer than 4000 characters",
                    ));
                }
            }
            if o.get("extra").is_some_and(|v| !v.is_object()) {
                return Err(invalid("option 'extra' must be a JSON object"));
            }
            if let Some(v) = o.get("voices_path") {
                match v.as_str() {
                    Some(p) if p.starts_with('/') && p.len() <= 200 && !p.contains(['?', '#']) => {}
                    _ => {
                        return Err(invalid(
                            "option 'voices_path' must be a path starting with '/'",
                        ))
                    }
                }
            }
        }
        Ok(())
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
    pub fn origin(&self) -> Option<String> {
        self.parsed_url().map(|u| u.origin())
    }

    /// The origin of the provider itself (the kind's or preset's default
    /// address, never the profile's `url`): an `env:` key may always go
    /// there.
    pub fn default_origin(&self) -> Option<String> {
        default_base(self.kind, &self.options)
            .and_then(|u| Url::parse(u.trim_end_matches('/')).ok())
            .map(|u| u.origin())
    }

    pub fn effective_model(&self) -> Option<String> {
        self.model.clone().or_else(|| match self.kind {
            Kind::OpenAiCompatible => self.preset().default_model().map(str::to_string),
            _ => None,
        })
    }

    pub fn effective_voice(&self) -> Option<String> {
        self.voice.clone().or_else(|| match self.kind {
            Kind::OpenAiCompatible => self.preset().default_voice().map(str::to_string),
            _ => None,
        })
    }

    /// The name spoken in cues and shown in lists.
    pub fn display_label(&self) -> String {
        if let Some(l) = &self.label {
            return l.clone();
        }
        match self.kind {
            Kind::OpenAiCompatible => self.preset().display_name().to_string(),
            k => k.as_str().to_string(),
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

    /// `sends_text_to` of the profile view (spec 10.1).
    pub fn sends_text_to(&self) -> String {
        self.host()
    }

    /// Request timeout (spec 5.4 common options).
    pub fn timeout_ms(&self) -> u64 {
        self.option_u64("timeout_ms").unwrap_or(match self.kind {
            Kind::Command => 30_000,
            _ if self.is_local() => 60_000,
            _ => 15_000,
        })
    }

    /// Chunks synthesized ahead (spec D10).
    pub fn prefetch(&self) -> usize {
        self.option_u64("prefetch")
            .map(|n| n.clamp(1, 4) as usize)
            .unwrap_or(if self.is_local() { 1 } else { 2 })
    }

    /// Whether the kind needs a key to work at all (a key_ref other than
    /// `none`).
    pub fn needs_key(&self) -> bool {
        self.key_ref != KeyRef::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: Value) -> Result<Profile, ProfileError> {
        Profile::from_json(&v)
    }

    fn err(v: Value) -> String {
        match parse(v) {
            Err(ProfileError::Invalid(m)) => m,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    fn local(extra: Value) -> Value {
        let mut v = json!({"id": "loc", "kind": "openai-compatible",
            "url": "http://127.0.0.1:8880/v1", "options": {"preset": "kokoro-fastapi"}});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v
    }

    #[test]
    fn ids_follow_the_rule() {
        for ok in ["a", "openai", "kokoro-gpu", "x_1", &"a".repeat(32)] {
            assert!(validate_id(ok).is_ok(), "{ok}");
        }
        for (bad, msg) in [
            ("", "invalid engine id"),
            ("-a", "invalid engine id"),
            ("A", "invalid engine id"),
            ("a b", "invalid engine id"),
            (&"a".repeat(33), "invalid engine id"),
            ("kokoro", "'kokoro' is a built-in engine"),
            ("onecore", "built-in"),
            ("fake", "built-in"),
            ("sonara-x", "reserved"),
        ] {
            assert!(validate_id(bad).unwrap_err().contains(msg), "{bad}");
        }
        assert!(
            err(local(json!({"id": "Bad"}))).starts_with("invalid engine id 'Bad': use 1 to 32")
        );
    }

    #[test]
    fn labels_models_and_voices_are_bounded_plain_text() {
        assert!(err(local(json!({"label": "x".repeat(41)}))).contains("label"));
        assert!(err(local(json!({"label": "a\nb"}))).contains("control"));
        assert!(parse(local(json!({"label": "x".repeat(40)}))).is_ok());
        assert!(err(local(json!({"model": "m".repeat(201)}))).contains("model"));
        assert!(err(local(json!({"voice": "v\u{7}"}))).contains("voice"));
        assert!(err(local(json!({"voice": 3}))).contains("'voice' must be a string"));
    }

    #[test]
    fn urls_need_https_unless_loopback_or_allowed() {
        let cloud = |url: &str, extra: Value| {
            let mut v = json!({"id": "c", "kind": "openai-compatible", "url": url,
                "options": {"preset": "generic"}});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        assert!(parse(cloud("https://tts.example.com/v1", json!({}))).is_ok());
        for local in [
            "http://localhost:8880/v1",
            "http://127.0.0.1/v1",
            "http://127.9.9.9:1/v1",
            "http://[::1]:8880/v1",
        ] {
            let p = parse(cloud(local, json!({}))).unwrap();
            assert!(p.is_local(), "{local}");
            assert_eq!(p.key_ref, KeyRef::None, "{local}: local default is no key");
        }
        assert!(err(cloud("http://10.0.0.5:8880/v1", json!({}))).contains("allow_http"));
        // allow_http lets plain http through, but never with a key.
        assert_eq!(
            err(cloud(
                "http://10.0.0.5:8880/v1",
                json!({"options": {"allow_http": true}})
            )),
            "a key is never sent over http to a non-loopback host"
        );
        assert!(parse(cloud(
            "http://10.0.0.5:8880/v1",
            json!({"options": {"allow_http": true}, "key_ref": "none"})
        ))
        .is_ok());
        for bad in [
            "ftp://x/v1",
            "https://user:pw@x/v1",
            "https://x/v1?key=1",
            "https://x/v1#f",
            "x/v1",
            "https://:80/v1",
        ] {
            assert!(err(cloud(bad, json!({}))).contains("invalid url"), "{bad}");
        }
    }

    #[test]
    fn key_refs_and_their_defaults() {
        let openai = parse(json!({"id": "openai", "kind": "openai-compatible",
            "options": {"preset": "openai"}}))
        .unwrap();
        assert_eq!(openai.key_ref, KeyRef::CredMan);
        assert_eq!(
            openai.base_url().as_deref(),
            Some("https://api.openai.com/v1")
        );
        let env = parse(local(json!({"key_ref": "env:MY_1_API_KEY"}))).unwrap();
        assert_eq!(env.key_ref, KeyRef::Env("MY_1_API_KEY".into()));
        assert_eq!(env.to_json()["key_ref"], "env:MY_1_API_KEY");
        assert!(err(local(json!({"key_ref": "env:1BAD"}))).contains("environment variable"));
        // Only a variable named like an API key (or Sonara's own) may be
        // sent: a client must not exfiltrate the runtime's other secrets.
        for ok in [
            "OPENAI_API_KEY",
            "openai_api_key",
            "SONARA_KEY",
            "AZURE_SPEECH_KEY",
            "SPEECH_KEY",
        ] {
            assert!(
                parse(local(json!({"key_ref": format!("env:{ok}")}))).is_ok(),
                "{ok}"
            );
        }
        for bad in ["GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "PATH", "API_KEY_X"] {
            assert!(
                err(local(json!({"key_ref": format!("env:{bad}")})))
                    .contains("must end in _API_KEY or _SPEECH_KEY, or start with SONARA_"),
                "{bad}"
            );
        }
        assert!(err(local(json!({"key_ref": "vault"}))).contains("unknown key_ref"));
        assert_eq!(
            parse(local(json!({"key_ref": null}))).unwrap().key_ref,
            KeyRef::None
        );
    }

    #[test]
    fn options_are_known_and_typed() {
        assert_eq!(
            err(local(
                json!({"options": {"preset": "kokoro-fastapi", "speeed": 1}})
            )),
            "unknown option 'speeed' for kind 'openai-compatible'"
        );
        for (opts, msg) in [
            (json!({"timeout_ms": 999}), "timeout_ms"),
            (json!({"timeout_ms": 120_001}), "timeout_ms"),
            (json!({"prefetch": 0}), "prefetch"),
            (json!({"prefetch": 5}), "prefetch"),
            (json!({"allow_http": "yes"}), "allow_http"),
            (json!({"preset": "acme"}), "unknown preset 'acme'"),
            (json!({"response_format": "mp3"}), "response_format"),
            (json!({"sample_rate": 7999}), "sample_rate"),
            (json!({"instructions": 5}), "instructions"),
            (json!({"extra": [1]}), "extra"),
            (json!({"voices_path": "voices"}), "voices_path"),
        ] {
            let mut o = opts.as_object().unwrap().clone();
            o.entry("preset").or_insert(json!("kokoro-fastapi"));
            assert!(err(local(json!({"options": o}))).contains(msg), "{opts}");
        }
        let ok = parse(local(json!({"options": {"preset": "kokoro-fastapi",
            "timeout_ms": 1000, "prefetch": 4, "allow_http": false, "response_format": "pcm",
            "sample_rate": 24000, "instructions": "calm", "extra": {"stream": false},
            "voices_path": "/audio/voices"}})))
        .unwrap();
        assert_eq!(ok.timeout_ms(), 1000);
        assert_eq!(ok.prefetch(), 4);
    }

    #[test]
    fn preset_requirements_and_defaults() {
        assert!(err(json!({"id": "k", "kind": "openai-compatible",
            "options": {"preset": "kokoro-fastapi"}}))
        .contains("needs a url"));
        assert!(err(json!({"id": "l", "kind": "openai-compatible",
            "url": "http://127.0.0.1:8080/v1", "options": {"preset": "localai"}}))
        .contains("needs a model"));
        assert!(err(json!({"id": "s", "kind": "openai-compatible",
            "url": "http://127.0.0.1:8000/v1", "options": {"preset": "speaches"}}))
        .contains("needs a model"));
        assert!(err(json!({"id": "c", "kind": "openai-compatible",
            "url": "http://127.0.0.1:8004/v1", "options": {"preset": "chatterbox-server"}}))
        .contains("needs a voice"));
        for (preset, model, voice) in [
            ("openai", Some("gpt-4o-mini-tts"), Some("marin")),
            ("kokoro-fastapi", Some("kokoro"), Some("af_heart")),
            ("openedai-speech", Some("tts-1"), Some("alloy")),
            ("chatterbox-api", Some("chatterbox"), Some("alloy")),
            ("generic", Some("tts-1"), Some("alloy")),
        ] {
            let p = parse(json!({"id": "p", "kind": "openai-compatible",
                "url": "http://127.0.0.1:1/v1", "options": {"preset": preset}}))
            .unwrap();
            assert_eq!(p.effective_model().as_deref(), model, "{preset}");
            assert_eq!(p.effective_voice().as_deref(), voice, "{preset}");
        }
        // No preset is generic.
        let p = parse(json!({"id": "g", "kind": "openai-compatible",
            "url": "https://tts.example.com/v1/"}))
        .unwrap();
        assert_eq!(p.preset(), Preset::Generic);
        assert_eq!(p.base_url().as_deref(), Some("https://tts.example.com/v1"));
    }

    #[test]
    fn defaults_for_cloud_and_local_profiles() {
        let cloud = parse(json!({"id": "openai", "kind": "openai-compatible",
            "options": {"preset": "openai"}}))
        .unwrap();
        assert_eq!((cloud.timeout_ms(), cloud.prefetch()), (15_000, 2));
        assert!(!cloud.is_local());
        assert_eq!(cloud.sends_text_to(), "api.openai.com");
        assert_eq!(cloud.display_label(), "OpenAI");
        let loc = parse(local(json!({}))).unwrap();
        assert_eq!((loc.timeout_ms(), loc.prefetch()), (60_000, 1));
        assert!(loc.is_local());
        assert_eq!(loc.display_label(), "Kokoro FastAPI");
    }

    #[test]
    fn unknown_kinds_are_unsupported_not_invalid() {
        assert_eq!(
            parse(json!({"id": "el", "kind": "elevenlabs", "voice": "x"})),
            Err(ProfileError::Unsupported {
                id: "el".into(),
                kind: "elevenlabs".into()
            })
        );
        assert!(matches!(
            parse(json!({"id": "x", "kind": "future-kind"})),
            Err(ProfileError::Unsupported { .. })
        ));
        assert_eq!(implemented_kinds(), vec![Kind::OpenAiCompatible]);
    }

    #[test]
    fn origins_include_the_port_and_the_kind_defaults() {
        let o = |v: Value| origin_of(&v);
        assert_eq!(
            o(json!({"kind": "openai-compatible", "options": {"preset": "openai"}})).as_deref(),
            Some("https://api.openai.com:443")
        );
        assert_eq!(
            o(json!({"kind": "openai-compatible", "url": "https://API.example.com/v1/"}))
                .as_deref(),
            Some("https://api.example.com:443")
        );
        assert_eq!(
            o(json!({"kind": "openai-compatible", "url": "http://127.0.0.1:8880/v1"})).as_deref(),
            Some("http://127.0.0.1:8880")
        );
        assert_eq!(
            o(json!({"kind": "openai-compatible", "url": "http://[::1]/v1"})).as_deref(),
            Some("http://[::1]:80")
        );
        assert_eq!(
            o(json!({"kind": "openai-compatible", "url": "https://x.example:8443"})).as_deref(),
            Some("https://x.example:8443")
        );
        // Kinds this build lacks still have an origin (their keys are
        // bound too): the provider's default or Azure's region.
        for (kind, want) in [
            ("elevenlabs", "https://api.elevenlabs.io:443"),
            ("google", "https://texttospeech.googleapis.com:443"),
            ("cartesia", "https://api.cartesia.ai:443"),
            ("deepgram", "https://api.deepgram.com:443"),
        ] {
            assert_eq!(o(json!({"kind": kind})).as_deref(), Some(want), "{kind}");
        }
        assert_eq!(
            o(json!({"kind": "azure", "options": {"region": "westeurope"}})).as_deref(),
            Some("https://westeurope.tts.speech.microsoft.com:443")
        );
        assert_eq!(
            o(json!({"kind": "azure", "options": {"region": "evil.example.com/x"}})),
            None,
            "a region is a host label, not an address"
        );
        assert_eq!(
            o(json!({"kind": "deepgram", "url": "https://api.eu.deepgram.com"})).as_deref(),
            Some("https://api.eu.deepgram.com:443")
        );
        assert_eq!(o(json!({"kind": "command"})), None);
        assert_eq!(
            o(json!({"kind": "openai-compatible", "url": "ftp://x"})),
            None
        );
        // A profile's origin and its provider default.
        let p = parse(json!({"id": "openai", "kind": "openai-compatible",
            "url": "https://proxy.example.com/v1", "options": {"preset": "openai"}}))
        .unwrap();
        assert_eq!(p.origin().as_deref(), Some("https://proxy.example.com:443"));
        assert_eq!(
            p.default_origin().as_deref(),
            Some("https://api.openai.com:443")
        );
        assert_eq!(parse(local(json!({}))).unwrap().default_origin(), None);
        assert_eq!(
            parse(
                json!({"id": "k", "kind": "openai-compatible", "key_origin": "https://evil:443",
                "url": "https://tts.example.com/v1"})
            )
            .unwrap()
            .key_origin,
            None,
            "key_origin never comes from the profile JSON"
        );
    }

    #[test]
    fn json_round_trip_drops_secret_fields() {
        let v = json!({"id": "openai", "kind": "openai-compatible", "label": "OpenAI",
            "url": "https://api.openai.com/v1", "model": "gpt-4o-mini-tts", "voice": "marin",
            "key_ref": "credman", "options": {"preset": "openai"},
            "secret": "sk-should-never-be-kept-1234", "api_key": "x"});
        let p = parse(v).unwrap();
        let out = p.to_json();
        assert!(out.get("secret").is_none() && out.get("api_key").is_none());
        assert_eq!(parse(out.clone()).unwrap(), p);
        assert_eq!(
            out,
            json!({"id": "openai", "kind": "openai-compatible", "label": "OpenAI",
                "url": "https://api.openai.com/v1", "model": "gpt-4o-mini-tts", "voice": "marin",
                "key_ref": "credman", "options": {"preset": "openai"}})
        );
    }
}
