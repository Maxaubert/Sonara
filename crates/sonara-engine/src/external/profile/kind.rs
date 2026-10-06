//! The provider shape of a profile (`Kind`), the servers behind
//! `openai-compatible` (`Preset`) and where a key comes from (`KeyRef`).
use serde_json::Value;

/// The provider shape of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    OpenAiCompatible,
    ElevenLabs,
    Azure,
    Google,
    Gemini,
    Cartesia,
    Deepgram,
    Command,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::OpenAiCompatible,
        Kind::ElevenLabs,
        Kind::Azure,
        Kind::Google,
        Kind::Gemini,
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
            Kind::Gemini => "gemini",
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
        // Every kind of `ALL` since PR3 (#226); a name this build does not
        // know is not a `Kind` at all, so it stays unsupported.
        true
    }

    /// Spoken in cues and shown when a profile has no label.
    pub fn display_name(&self) -> &'static str {
        match self {
            Kind::OpenAiCompatible => "The speech server",
            Kind::ElevenLabs => "ElevenLabs",
            Kind::Azure => "Azure Speech",
            Kind::Google => "Google Text-to-Speech",
            Kind::Gemini => "Gemini",
            Kind::Cartesia => "Cartesia",
            Kind::Deepgram => "Deepgram",
            Kind::Command => "The speech program",
        }
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

    /// Whether the server needs a model in each request (OpenAI's API,
    /// LocalAI and Speaches host several): the profile must name one.
    /// Another server picks its own when Sonara sends none (Chatterbox-
    /// TTS-Server, whose schema requires the field but never reads it,
    /// gets an empty one).
    pub fn model_required(&self) -> bool {
        matches!(self, Preset::OpenAi | Preset::LocalAi | Preset::Speaches)
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

pub(super) fn env_name_ok(n: &str) -> bool {
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
