//! The rules of spec 5.2 and the kinds' options of 5.4: `validate_id`,
//! `Profile::validate` and `Profile::check_new`.
use super::kind::env_name_ok;
use super::{
    invalid, KeyRef, Kind, Preset, Profile, ProfileError, Url, AZURE_FORMATS, BUILTIN_IDS,
    CARTESIA_RATES, CHUNK_CHARS_MAX, CHUNK_CHARS_MIN, COMMAND_MAX_ARGS, COMMAND_MAX_VOICES,
    DEEPGRAM_RATES, ELEVENLABS_FORMATS,
};
use serde_json::{Map, Value};

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

/// Options every kind accepts.
const COMMON_OPTIONS: &[&str] = &[
    "timeout_ms",
    "prefetch",
    "allow_http",
    "chunk_chars",
    "first_audio_ms",
];
/// Options of `elevenlabs`.
const ELEVENLABS_OPTIONS: &[&str] = &[
    "output_format",
    "stability",
    "similarity_boost",
    "style",
    "language_code",
    "enable_logging",
];
/// Options of `azure`.
const AZURE_OPTIONS: &[&str] = &["region", "output_format", "lang"];
/// Options of `google`.
const GOOGLE_OPTIONS: &[&str] = &["language_code", "sample_rate", "user_project", "model_name"];
/// Options of `gemini`.
const GEMINI_OPTIONS: &[&str] = &["language_code", "style"];
/// Options of `cartesia`.
const CARTESIA_OPTIONS: &[&str] = &["api_version", "language", "sample_rate"];
/// Options of `deepgram`.
const DEEPGRAM_OPTIONS: &[&str] = &["sample_rate"];
/// Options of `command`.
const COMMAND_OPTIONS: &[&str] = &["argv", "input", "output", "sample_rate", "voices"];
/// Options of `openai-compatible`.
const OPENAI_OPTIONS: &[&str] = &[
    "preset",
    "response_format",
    "sample_rate",
    "instructions",
    "extra",
    "voices_path",
];

pub(super) fn opt_text(
    m: &Map<String, Value>,
    field: &str,
) -> Result<Option<String>, ProfileError> {
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

fn num_in(o: &Map<String, Value>, field: &str, lo: f64, hi: f64) -> Result<(), ProfileError> {
    match o.get(field) {
        None => Ok(()),
        Some(v) => match v.as_f64() {
            Some(n) if (lo..=hi).contains(&n) => Ok(()),
            _ => Err(invalid(format!(
                "option '{field}' must be a number from {lo} to {hi}"
            ))),
        },
    }
}

/// A string option of 1 to `max` characters, each `allowed`.
fn word(
    o: &Map<String, Value>,
    field: &str,
    max: usize,
    allowed: fn(char) -> bool,
    what: &str,
) -> Result<(), ProfileError> {
    match o.get(field) {
        None => Ok(()),
        Some(v) => match v.as_str() {
            Some(s) if !s.is_empty() && s.chars().count() <= max && s.chars().all(allowed) => {
                Ok(())
            }
            _ => Err(invalid(format!("option '{field}' must be {what}"))),
        },
    }
}

fn lang_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
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
        }
        // A missing model or voice is not a validation error (#235): a
        // profile stored before Sonara stopped baking in defaults stays
        // usable and says "choose a model" or "choose a voice"
        // (`missing_model`, `External::status`) until the user picks one.
        let kind = self.kind.as_str();
        match self.kind {
            Kind::Azure if self.url.is_none() && self.option_str("region").is_none() => {
                return Err(invalid("kind 'azure' needs options.region or url"));
            }
            Kind::Azure | Kind::Google | Kind::Deepgram | Kind::Command if self.model.is_some() => {
                let hint = match self.kind {
                    Kind::Google => " (a Gemini TTS model goes in options.model_name)",
                    Kind::Deepgram => " (the voice is the model: pick it as the voice)",
                    _ => "",
                };
                return Err(invalid(format!("kind '{kind}' takes no model{hint}")));
            }
            Kind::Command if self.url.is_some() => {
                return Err(invalid(
                    "kind 'command' takes no url (the program goes in options.argv)",
                ));
            }
            _ => {}
        }
        Ok(())
    }

    /// The checks of a profile being added (not one loaded from
    /// `engines.json`): a `command` program must exist now, so a typo is
    /// caught at once, while a stored profile whose program is gone stays
    /// registered and reads with the fallback (`bad_config`) until it is
    /// back.
    pub fn check_new(&self) -> Result<(), ProfileError> {
        if let Some(program) = self.command_program() {
            if !std::path::Path::new(program).is_file() {
                return Err(invalid(format!("the program '{program}' does not exist")));
            }
        }
        Ok(())
    }
    fn validate_options(&self) -> Result<(), ProfileError> {
        let o = &self.options;
        let kind_opts: &[&str] = match self.kind {
            Kind::OpenAiCompatible => OPENAI_OPTIONS,
            Kind::ElevenLabs => ELEVENLABS_OPTIONS,
            Kind::Azure => AZURE_OPTIONS,
            Kind::Google => GOOGLE_OPTIONS,
            Kind::Gemini => GEMINI_OPTIONS,
            Kind::Cartesia => CARTESIA_OPTIONS,
            Kind::Deepgram => DEEPGRAM_OPTIONS,
            Kind::Command => COMMAND_OPTIONS,
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
        int_in(o, "first_audio_ms", 1000..=60_000)?;
        int_in(o, "chunk_chars", CHUNK_CHARS_MIN..=CHUNK_CHARS_MAX)?;
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
        self.validate_cloud_options()?;
        self.validate_more_options()
    }

    /// The options of `cartesia`, `deepgram` and `command` (spec 5.4).
    fn validate_more_options(&self) -> Result<(), ProfileError> {
        let o = &self.options;
        let rate_in = |rates: &[u64]| -> Result<(), ProfileError> {
            match o.get("sample_rate") {
                None => Ok(()),
                Some(v) if v.as_u64().is_some_and(|r| rates.contains(&r)) => Ok(()),
                Some(_) => {
                    let all: Vec<String> = rates.iter().map(u64::to_string).collect();
                    Err(invalid(format!(
                        "option 'sample_rate' must be one of {}",
                        all.join(", ")
                    )))
                }
            }
        };
        match self.kind {
            Kind::Cartesia => {
                word(
                    o,
                    "api_version",
                    10,
                    |c| c.is_ascii_digit() || c == '-',
                    "a date such as 2026-08-14",
                )?;
                word(o, "language", 16, lang_char, "a language code such as en")?;
                rate_in(CARTESIA_RATES)?;
            }
            Kind::Deepgram => rate_in(DEEPGRAM_RATES)?,
            Kind::Command => self.validate_command()?,
            Kind::Gemini => self.validate_gemini()?,
            _ => {}
        }
        Ok(())
    }

    /// The model and options of `gemini` (#235).
    fn validate_gemini(&self) -> Result<(), ProfileError> {
        let o = &self.options;
        if let Some(m) = &self.model {
            let ok = m
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
            if !ok || m.chars().count() > 100 {
                return Err(invalid(
                    "'model' must be a Gemini model id (letters, digits, '-', '.' and '_')",
                ));
            }
        }
        word(o, "language_code", 35, lang_char, "a locale such as en-US")?;
        if let Some(v) = o.get("style") {
            let s = v
                .as_str()
                .ok_or_else(|| invalid("option 'style' must be a string"))?;
            if s.chars().count() > 500 || s.chars().any(char::is_control) {
                return Err(invalid(
                    "option 'style' must be at most 500 characters on one line",
                ));
            }
        }
        Ok(())
    }

    /// `argv`, `input`, `output`, `sample_rate` and `voices` of `command`.
    fn validate_command(&self) -> Result<(), ProfileError> {
        let o = &self.options;
        let argv = match o.get("argv") {
            Some(Value::Array(a)) if !a.is_empty() => a,
            _ => {
                return Err(invalid(
                    "kind 'command' needs options.argv: the full path of the program \
                     (an .exe) and its arguments",
                ))
            }
        };
        if argv.len() > COMMAND_MAX_ARGS {
            return Err(invalid(format!(
                "option 'argv' has more than {COMMAND_MAX_ARGS} entries"
            )));
        }
        let mut args = Vec::with_capacity(argv.len());
        for a in argv {
            match a.as_str() {
                Some(s) if s.chars().count() <= 1000 && !s.contains('\0') => args.push(s),
                _ => {
                    return Err(invalid(
                        "option 'argv' must be a list of texts of at most 1000 characters",
                    ))
                }
            }
        }
        let program = std::path::Path::new(args[0]);
        if !program.is_absolute() || args[0].contains('{') {
            return Err(invalid(format!(
                "argv[0] '{}' must be the full path of the program, such as C:\\Tools\\tts.exe",
                args[0]
            )));
        }
        let exe = program
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        if !exe {
            return Err(invalid(format!(
                "argv[0] '{}' must be an .exe (a .bat or .cmd runs through the command \
                 shell, which would read the text as commands)",
                args[0]
            )));
        }
        let has = |p: &str| args[1..].iter().any(|a| a.contains(p));
        // The text read is never put on the command line (security review
        // of PR3): it goes on stdin or in a temporary file at {in}.
        if has("{text}") {
            return Err(invalid(
                "{text} is not allowed in argv: the text is never put on the command line; \
                 the program reads it on stdin (option input 'stdin', the default) or from \
                 the file at {in} (option input 'file')",
            ));
        }
        let input = o.get("input").map(|v| v.as_str().unwrap_or_default());
        match input {
            None | Some("stdin") => {}
            Some("file") if has("{in}") => {}
            Some("file") => return Err(invalid(
                "option input 'file' needs {in} in argv (the UTF-8 text file the program reads)",
            )),
            Some(_) => return Err(invalid("option 'input' must be \"stdin\" or \"file\"")),
        }
        if input != Some("file") && has("{in}") {
            return Err(invalid("{in} in argv needs option input 'file'"));
        }
        let output = o.get("output").map(|v| v.as_str().unwrap_or_default());
        match output {
            None | Some("stdout-wav") => {}
            Some("stdout-pcm") if o.contains_key("sample_rate") => {}
            Some("stdout-pcm") => {
                return Err(invalid(
                    "option output 'stdout-pcm' needs options.sample_rate (the program's rate)",
                ))
            }
            Some("file") if has("{out}") => {}
            Some("file") => {
                return Err(invalid(
                    "option output 'file' needs {out} in argv (the WAV file the program writes)",
                ))
            }
            Some(_) => {
                return Err(invalid(
                    "option 'output' must be \"stdout-wav\", \"stdout-pcm\" or \"file\"",
                ))
            }
        }
        if output != Some("file") && has("{out}") {
            return Err(invalid("{out} in argv needs option output 'file'"));
        }
        int_in(o, "sample_rate", 8000..=48_000)?;
        if let Some(v) = o.get("voices") {
            let list = v
                .as_array()
                .filter(|a| a.len() <= COMMAND_MAX_VOICES)
                .ok_or_else(|| {
                    invalid(format!(
                        "option 'voices' must be a list of at most {COMMAND_MAX_VOICES} voice names"
                    ))
                })?;
            for item in list {
                match item.as_str() {
                    Some(s) if !s.is_empty() => plain_text("voices", s, 200)?,
                    _ => return Err(invalid("option 'voices' must be a list of voice names")),
                }
            }
        }
        Ok(())
    }

    /// The options of `elevenlabs`, `azure` and `google` (spec 5.4).
    fn validate_cloud_options(&self) -> Result<(), ProfileError> {
        let o = &self.options;
        match self.kind {
            Kind::ElevenLabs => {
                if let Some(v) = o.get("output_format") {
                    if !v.as_str().is_some_and(|f| ELEVENLABS_FORMATS.contains(&f)) {
                        return Err(invalid(format!(
                            "option 'output_format' must be one of {}",
                            ELEVENLABS_FORMATS.join(", ")
                        )));
                    }
                }
                for f in ["stability", "similarity_boost", "style"] {
                    num_in(o, f, 0.0, 1.0)?;
                }
                word(
                    o,
                    "language_code",
                    16,
                    lang_char,
                    "a language code such as en",
                )?;
                if o.get("enable_logging").is_some_and(|v| !v.is_boolean()) {
                    return Err(invalid("option 'enable_logging' must be true or false"));
                }
            }
            Kind::Azure => {
                word(
                    o,
                    "region",
                    40,
                    |c| c.is_ascii_lowercase() || c.is_ascii_digit(),
                    "an Azure region such as westeurope (a-z, 0-9)",
                )?;
                if let Some(v) = o.get("output_format") {
                    if !v
                        .as_str()
                        .is_some_and(|f| AZURE_FORMATS.iter().any(|(n, _)| *n == f))
                    {
                        let all: Vec<&str> = AZURE_FORMATS.iter().map(|(n, _)| *n).collect();
                        return Err(invalid(format!(
                            "option 'output_format' must be one of {}",
                            all.join(", ")
                        )));
                    }
                }
                word(o, "lang", 35, lang_char, "a locale such as en-US")?;
            }
            Kind::Google => {
                word(o, "language_code", 35, lang_char, "a locale such as en-US")?;
                int_in(o, "sample_rate", 8000..=48_000)?;
                word(
                    o,
                    "user_project",
                    100,
                    |c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_'),
                    "a Google Cloud project id",
                )?;
                word(
                    o,
                    "model_name",
                    100,
                    |c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '/'),
                    "a model id (letters, digits, '-', '.', '_' and '/')",
                )?;
            }
            _ => {}
        }
        Ok(())
    }
}
