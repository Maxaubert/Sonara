//! Kind `openai-compatible` (spec 5.4, 13.1, 13.2): `POST {url}/audio/speech`
//! for OpenAI and the local servers that copy its API (Kokoro-FastAPI,
//! LocalAI, Speaches, openedai-speech, the two Chatterbox servers). WAV is
//! asked for by default (D8); the body is sniffed anyway.
//!
//! No model id and no voice in code (#235): models and voices change
//! upstream. The model list is `GET {url}/models` (for OpenAI the ids that
//! name `tts`); a server that picks its own model gets none when the
//! profile names none. OpenAI and openedai-speech have no voice list API,
//! so their voice is typed in (the settings page links the provider's
//! voice page); the other servers list theirs.
use super::adapter::{
    encode, key_allowed, Adapter, ErrorBody, HttpReply, HttpRequest, ModelInfo, ModelSource,
    VoiceInfo, VoiceSource,
};
use super::error::{clean, headline, model_message, ExtError};
use super::keys::Secret;
use super::profile::{Kind, Preset, Profile};
use super::rate;
use super::split::Limit;
use crate::Reason;
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicBool, Ordering};

/// The field a model may refuse (OpenAI's older speech models take no
/// `instructions`), as `ExtError::refused_param` names it.
const INSTRUCTIONS: &str = "instructions";

/// 429 codes that mean the account is out of credit, not busy.
const QUOTA_CODES: &[&str] = &[
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "organization_usage_limit_exceeded",
    "insufficient_quota",
];

pub struct OpenAi {
    preset: Preset,
    base: String,
    root: String,
    /// `None`: the server picks its own (or, where one is required, the
    /// profile says "choose a model").
    model: Option<String>,
    response_format: &'static str,
    sample_rate: Option<u32>,
    instructions: Option<String>,
    extra: Map<String, Value>,
    voices_path: Option<String>,
    label: String,
    /// The model refused `instructions`: they are not sent again.
    no_instructions: AtomicBool,
}

impl OpenAi {
    /// From a validated profile of kind `openai-compatible`.
    pub fn new(p: &Profile) -> OpenAi {
        let preset = p.preset();
        let base = p.base_url().unwrap_or_default();
        let root = base
            .strip_suffix("/v1")
            .map(str::to_string)
            .unwrap_or_else(|| base.clone());
        let response_format = match (preset, p.option_str("response_format")) {
            // Chatterbox-TTS-Server refuses pcm (422).
            (Preset::ChatterboxServer, _) => "wav",
            (_, Some("pcm")) => "pcm",
            _ => "wav",
        };
        let sample_rate = p.option_u64("sample_rate").map(|r| r as u32);
        OpenAi {
            preset,
            base,
            root,
            model: p.model.clone(),
            response_format,
            sample_rate,
            instructions: p.option_str("instructions").map(str::to_string),
            extra: p
                .options
                .get("extra")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            voices_path: p.option_str("voices_path").map(str::to_string),
            label: p.display_label(),
            no_instructions: AtomicBool::new(false),
        }
    }

    fn with_key(&self, mut req: HttpRequest, key: Option<&Secret>) -> HttpRequest {
        if let Some(k) = key.filter(|_| key_allowed(&req.url)) {
            req = req.header("Authorization", &format!("Bearer {}", k.expose()));
        }
        req
    }

    /// Whether `instructions` goes in the body: Kokoro-FastAPI and the
    /// Chatterbox servers do not take it; LocalAI forwards it to its
    /// expressive backends, so any other server gets it when it is set. A
    /// model that refuses it (OpenAI's older speech models) gets it no
    /// more (`adapt`): Sonara keeps no list of which models take it.
    fn takes_instructions(&self) -> bool {
        if self.no_instructions.load(Ordering::SeqCst) {
            return false;
        }
        match self.preset {
            Preset::KokoroFastApi | Preset::ChatterboxApi | Preset::ChatterboxServer => false,
            Preset::OpenAi
            | Preset::LocalAi
            | Preset::Speaches
            | Preset::OpenedAiSpeech
            | Preset::Generic => true,
        }
    }

    /// The request body (tests check it per preset).
    pub fn body(&self, text: &str, voice: &str, wpm: u32) -> Value {
        let mut b = Map::new();
        match &self.model {
            Some(m) => {
                b.insert("model".into(), json!(m));
            }
            // Chatterbox-TTS-Server's request schema requires `model`, which
            // its route never reads (#275): an empty one keeps the request
            // valid with no model name in code (#235).
            None if self.preset == Preset::ChatterboxServer => {
                b.insert("model".into(), json!(""));
            }
            None => {}
        }
        b.insert("input".into(), json!(text));
        let voice_value = if self.preset == Preset::OpenAi && voice.starts_with("voice_") {
            json!({"id": voice})
        } else {
            json!(voice)
        };
        b.insert("voice".into(), voice_value);
        b.insert("response_format".into(), json!(self.response_format));
        if let Some(s) = rate::speed(Kind::OpenAiCompatible, wpm) {
            b.insert("speed".into(), json!(s));
        }
        if let Some(i) = &self.instructions {
            if self.takes_instructions() {
                b.insert("instructions".into(), json!(i));
            }
        }
        if self.preset == Preset::KokoroFastApi {
            b.insert("stream".into(), json!(false));
        }
        if self.preset == Preset::Speaches {
            b.insert(
                "sample_rate".into(),
                json!(self.sample_rate.unwrap_or(24_000)),
            );
        }
        for (k, v) in &self.extra {
            b.insert(k.clone(), v.clone());
        }
        Value::Object(b)
    }

    /// Whether the refusal is about the model (an unknown or retired one).
    fn about_model(&self, s: u16, eb: &ErrorBody, code: &str) -> bool {
        self.model.is_some()
            && (code == "model_not_found"
                || eb.param.as_deref() == Some("model")
                || (matches!(s, 400 | 404 | 422)
                    && eb.mentions("model")
                    && !eb.mentions("voice")
                    && [
                        "not found",
                        "does not exist",
                        "not supported",
                        "unsupported",
                        "unknown",
                        "deprecated",
                    ]
                    .iter()
                    .any(|p| eb.mentions(p))))
    }
}

impl Adapter for OpenAi {
    fn input_limit(&self) -> Limit {
        match self.preset {
            Preset::ChatterboxApi => Limit::Chars(3000),
            _ => Limit::Chars(4096),
        }
    }

    fn synth_request(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
    ) -> HttpRequest {
        let req = HttpRequest::post_json(
            format!("{}/audio/speech", self.base),
            &self.body(text, voice, wpm),
        );
        self.with_key(req, key)
    }

    /// Raw `pcm` comes as it is made (OpenAI's chunked answer, #235); WAV
    /// is waited for whole.
    fn bytes_request(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
    ) -> Option<HttpRequest> {
        self.raw_pcm()
            .then(|| self.synth_request(text, voice, wpm, key))
    }

    fn requested_rate(&self) -> Option<u32> {
        Some(self.sample_rate.unwrap_or(24_000))
    }

    fn raw_pcm(&self) -> bool {
        self.response_format == "pcm"
    }

    fn map_error(&self, reply: &HttpReply, _voice: &str, _listed: Option<bool>) -> ExtError {
        let s = reply.status;
        let eb = ErrorBody::parse(&reply.body);
        let code = eb.code.clone().or(eb.kind.clone()).unwrap_or_default();
        let reason = match s {
            401 | 403 => Reason::Auth,
            402 => Reason::Quota,
            429 if QUOTA_CODES.contains(&code.as_str()) => Reason::Quota,
            429 => Reason::RateLimited,
            400 | 404 | 422 if eb.mentions("voice") => Reason::BadVoice,
            400 | 404 | 415 | 422 => Reason::BadConfig,
            500..=599 => Reason::Server,
            400..=499 => Reason::BadConfig,
            _ => Reason::Server,
        };
        let text = if reason == Reason::BadConfig && self.about_model(s, &eb, &code) {
            model_message(
                &self.label,
                self.model.as_deref().unwrap_or_default(),
                s,
                &eb.text_or(s),
            )
        } else {
            format!(
                "{} ({s}): {}",
                headline(reason, &self.label, ""),
                eb.text_or(s)
            )
        };
        let mut e = ExtError::new(reason, text).with_status(s);
        if matches!(s, 429 | 503) {
            e.retry_after = reply.retry_after;
        }
        if reason == Reason::BadConfig
            && (eb.param.as_deref() == Some(INSTRUCTIONS) || eb.mentions(INSTRUCTIONS))
        {
            e.refused_param = Some(INSTRUCTIONS);
        }
        e
    }

    /// `instructions` refused: the part once more without them, and never
    /// again with them.
    fn adapt(&self, request: &HttpRequest, error: &ExtError) -> bool {
        let carried = request
            .body
            .as_deref()
            .is_some_and(|b| String::from_utf8_lossy(b).contains("\"instructions\""));
        error.refused_param == Some(INSTRUCTIONS)
            && carried
            && !self.no_instructions.swap(true, Ordering::SeqCst)
    }

    /// `GET {url}/models`: for OpenAI the ids that name `tts`; for a
    /// server, the models it marks as text-to-speech, else all it lists
    /// (a server without the list is an empty one: the model is typed).
    fn models(&self, key: Option<&Secret>) -> Option<ModelSource> {
        Some(ModelSource {
            request: self.with_key(HttpRequest::get(format!("{}/models", self.base)), key),
            empty_on_error: self.preset != Preset::OpenAi,
        })
    }

    fn parse_models(&self, body: &[u8]) -> Result<Vec<ModelInfo>, ExtError> {
        let v: Value = serde_json::from_slice(body).map_err(|e| {
            ExtError::new(
                Reason::Format,
                format!(
                    "the model list of {} is not JSON: {}",
                    self.label,
                    clean(&e.to_string())
                ),
            )
        })?;
        let entries = v
            .get("data")
            .or_else(|| v.get("models"))
            .and_then(Value::as_array)
            .or_else(|| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or_default();
        let mut out: Vec<ModelInfo> = entries
            .iter()
            .filter_map(|m| {
                let id = match m {
                    Value::String(s) => s.clone(),
                    _ => m.get("id").and_then(Value::as_str)?.to_string(),
                };
                let task = m
                    .get("task")
                    .or_else(|| m.get("type"))
                    .and_then(Value::as_str)
                    .map(str::to_ascii_lowercase);
                let speech = match (&task, self.preset) {
                    (_, Preset::OpenAi) => id.to_ascii_lowercase().contains("tts"),
                    (Some(t), _) => t.contains("text-to-speech") || t.contains("tts"),
                    (None, _) => true,
                };
                speech.then(|| ModelInfo::named(&id))
            })
            .collect();
        let mut seen = std::collections::HashSet::new();
        out.retain(|m| seen.insert(m.id.clone()));
        Ok(out)
    }

    fn voices(&self, key: Option<&Secret>) -> VoiceSource {
        let get = |url: String| self.with_key(HttpRequest::get(url), key);
        let fetch = |url: String| VoiceSource::Fetch {
            request: get(url),
            empty_on_error: false,
        };
        match self.preset {
            // No voice list API: the voice is typed in (none in code).
            Preset::OpenAi | Preset::OpenedAiSpeech => VoiceSource::Fixed(Vec::new()),
            Preset::KokoroFastApi | Preset::Speaches => {
                fetch(format!("{}/audio/voices", self.base))
            }
            Preset::LocalAi => fetch(match &self.model {
                Some(m) => format!("{}/audio/voices?model={}", self.base, encode(m)),
                None => format!("{}/audio/voices", self.base),
            }),
            Preset::ChatterboxApi => fetch(format!("{}/voices", self.root)),
            Preset::ChatterboxServer => fetch(format!("{}/get_predefined_voices", self.root)),
            Preset::Generic => VoiceSource::Fetch {
                request: get(format!(
                    "{}{}",
                    self.base,
                    self.voices_path.as_deref().unwrap_or("/audio/voices")
                )),
                empty_on_error: true,
            },
        }
    }

    fn parse_voices(&self, body: &[u8]) -> Result<Vec<VoiceInfo>, ExtError> {
        let v: Value = serde_json::from_slice(body).map_err(|e| {
            ExtError::new(
                Reason::Format,
                format!(
                    "the voice list of {} is not JSON: {}",
                    self.label,
                    clean(&e.to_string())
                ),
            )
        })?;
        Ok(parse_voice_list(&v))
    }
}

/// Any of the list shapes of spec 5.4: `{"voices": [..]}`, LocalAI's
/// `{"data": [{"voices": [..]}]}`, or a top-level array; entries are
/// strings or objects (`id`, `voice_id`, `name` or `filename`). The name
/// comes before the file name: Chatterbox TTS API lists both and its
/// speech route takes the name (a file name there silently speaks the
/// default voice, #275); Chatterbox-TTS-Server lists only `filename`.
pub fn parse_voice_list(v: &Value) -> Vec<VoiceInfo> {
    let entries: Vec<&Value> = if let Some(a) = v.get("voices").and_then(Value::as_array) {
        a.iter().collect()
    } else if let Some(d) = v.get("data").and_then(Value::as_array) {
        d.iter()
            .flat_map(|m| {
                m.get("voices")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().collect::<Vec<_>>())
                    .unwrap_or_else(|| vec![m])
            })
            .collect()
    } else if let Some(a) = v.as_array() {
        a.iter().collect()
    } else {
        Vec::new()
    };
    let s = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
    let mut out: Vec<VoiceInfo> = entries
        .into_iter()
        .filter_map(|e| match e {
            Value::String(id) => Some(VoiceInfo::named(id)),
            Value::Object(_) => {
                let id = s(e, "id")
                    .or_else(|| s(e, "voice_id"))
                    .or_else(|| s(e, "name"))
                    .or_else(|| s(e, "filename"))?;
                let name = s(e, "display_name")
                    .or_else(|| s(e, "name"))
                    .unwrap_or_else(|| id.clone());
                let language = s(e, "language")
                    .or_else(|| s(e, "lang"))
                    .or_else(|| {
                        e.get("languages")
                            .and_then(Value::as_array)
                            .and_then(|l| l.first())
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                Some(VoiceInfo { id, name, language })
            }
            _ => None,
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    out.retain(|v| seen.insert(v.id.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(v: Value) -> OpenAi {
        OpenAi::new(&Profile::from_json(&v).unwrap())
    }

    fn preset(name: &str, extra: Value) -> OpenAi {
        let mut v = json!({"id": "p", "kind": "openai-compatible", "url": "http://127.0.0.1:9/v1",
            "model": "m1", "options": {"preset": name}});
        for (k, x) in extra.as_object().unwrap() {
            if k == "options" {
                for (ok, ov) in x.as_object().unwrap() {
                    v["options"][ok] = ov.clone();
                }
            } else {
                v[k] = x.clone();
            }
        }
        adapter(v)
    }

    fn reply(status: u16, body: &str) -> HttpReply {
        HttpReply {
            status,
            content_type: Some("application/json".into()),
            retry_after: None,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn error_mapping_follows_the_table() {
        let a = preset("generic", json!({}));
        let err = |s, b: &str| a.map_error(&reply(s, b), "v", None).reason;
        let openai = |code: &str| {
            format!(r#"{{"error": {{"message": "m", "type": "t", "code": "{code}"}}}}"#)
        };
        assert_eq!(err(401, &openai("invalid_api_key")), Reason::Auth);
        assert_eq!(
            err(403, &openai("unsupported_country_region_territory")),
            Reason::Auth
        );
        for q in QUOTA_CODES {
            assert_eq!(err(429, &openai(q)), Reason::Quota, "{q}");
        }
        assert_eq!(
            err(
                429,
                r#"{"error": {"message": "m", "type": "insufficient_quota"}}"#
            ),
            Reason::Quota
        );
        assert_eq!(err(429, &openai("slow_down")), Reason::RateLimited);
        assert_eq!(
            err(
                429,
                r#"{"error": {"code": 429, "message": "x", "type": "rate_limit_error"}}"#
            ),
            Reason::RateLimited
        );
        assert_eq!(
            err(
                400,
                r#"{"error": {"message": "Invalid value for 'voice'", "param": "voice"}}"#
            ),
            Reason::BadVoice
        );
        assert_eq!(
            err(404, r#"{"detail": "Voice file 'Bob.wav' not found"}"#),
            Reason::BadVoice
        );
        assert_eq!(
            err(
                422,
                r#"{"detail": [{"loc": ["body", "voice"], "msg": "bad"}]}"#
            ),
            Reason::BadVoice
        );
        assert_eq!(err(400, &openai("model_not_found")), Reason::BadConfig);
        // An unknown or retired model is named (#235).
        let e = a.map_error(&reply(404, &openai("model_not_found")), "v", None);
        assert!(
            e.message
                .starts_with("The speech server does not know the model 'm1' (404)"),
            "{}",
            e.message
        );
        assert!(e.message.contains("Choose another model"));
        let e = a.map_error(
            &reply(400, r#"{"detail": "Model 'm1' not found"}"#),
            "v",
            None,
        );
        assert!(e.message.contains("model 'm1'"), "{}", e.message);
        // A voice error stays a voice error.
        let e = a.map_error(
            &reply(404, r#"{"detail": "Voice 'x' not found for model m1"}"#),
            "x",
            None,
        );
        assert_eq!(e.reason, Reason::BadVoice);
        assert_eq!(err(404, r#"{"detail": "Not Found"}"#), Reason::BadConfig);
        assert_eq!(err(415, ""), Reason::BadConfig);
        assert_eq!(
            err(422, r#"{"detail": "pcm not supported"}"#),
            Reason::BadConfig
        );
        assert_eq!(err(402, ""), Reason::Quota);
        for s in [500, 502, 504, 503] {
            assert_eq!(
                err(s, r#"{"error": {"message": "server_is_overloaded"}}"#),
                Reason::Server
            );
        }
    }

    #[test]
    fn error_messages_name_the_cause_and_mask_keys() {
        let a = adapter(json!({"id": "openai", "kind": "openai-compatible",
            "options": {"preset": "openai"}}));
        let e = a.map_error(
            &reply(
                401,
                r#"{"error": {"message": "Incorrect API key provided: sk-proj-abcdefghijklmnopqrst"}}"#,
            ),
            "v1",
            None,
        );
        assert_eq!(e.status, Some(401));
        assert_eq!(
            e.message,
            "OpenAI refused the key (401): Incorrect API key provided: [redacted]"
        );
        let mut r = reply(429, r#"{"error": {"code": "slow_down"}}"#);
        r.retry_after = Some(std::time::Duration::from_secs(1));
        assert_eq!(
            a.map_error(&r, "v", None).retry_after,
            Some(std::time::Duration::from_secs(1))
        );
    }

    #[test]
    fn request_bodies_per_preset() {
        let openai = adapter(
            json!({"id": "o", "kind": "openai-compatible", "model": "m1",
            "options": {"preset": "openai", "instructions": "Calm."}}),
        );
        assert_eq!(
            openai.body("Hi.", "v1", 250),
            json!({"model": "m1", "input": "Hi.", "voice": "v1",
                "response_format": "wav", "speed": 1.25, "instructions": "Calm."})
        );
        assert_eq!(
            openai.body("Hi.", "voice_abc123", 200)["voice"],
            json!({"id": "voice_abc123"})
        );
        // A model that refuses instructions gets them no more (no list of
        // models in code, #235).
        let req = openai.synth_request("Hi.", "v1", 200, None);
        let e = openai.map_error(
            &reply(
                400,
                r#"{"error": {"message": "instructions is not supported with this model", "param": "instructions"}}"#,
            ),
            "v1",
            None,
        );
        assert_eq!(e.refused_param, Some("instructions"));
        assert!(openai.adapt(&req, &e));
        assert!(openai.body("Hi.", "v1", 200).get("instructions").is_none());
        assert!(!openai.adapt(&req, &e), "once");
        // No model named: none sent, the server picks its own.
        let own = preset("kokoro-fastapi", json!({}));
        let mut v = json!({"id": "p", "kind": "openai-compatible", "url": "http://127.0.0.1:9/v1",
            "options": {"preset": "kokoro-fastapi"}});
        let bare = adapter(v.take());
        assert!(bare.body("Hi.", "v1", 200).get("model").is_none());
        assert_eq!(own.body("Hi.", "v1", 200)["model"], "m1");
        // LocalAI forwards instructions to expressive backends; a server
        // known not to take them never gets them.
        let calm = json!({"voice": "Emily.wav", "options": {"instructions": "Calm."}});
        for (name, sent) in [
            ("localai", true),
            ("generic", true),
            ("speaches", true),
            ("kokoro-fastapi", false),
            ("chatterbox-api", false),
            ("chatterbox-server", false),
        ] {
            let b = preset(name, calm.clone()).body("Hi.", "v", 200);
            assert_eq!(b.get("instructions").is_some(), sent, "{name}");
        }
        let kokoro = preset("kokoro-fastapi", json!({}));
        assert_eq!(kokoro.body("a", "v1", 200)["stream"], json!(false));
        let speaches = preset("speaches", json!({}));
        assert_eq!(speaches.body("a", "v1", 200)["sample_rate"], json!(24_000));
        let speaches = preset("speaches", json!({"options": {"sample_rate": 16000}}));
        assert_eq!(speaches.body("a", "x", 200)["sample_rate"], json!(16_000));
        let generic = preset(
            "generic",
            json!({"options": {"response_format": "pcm",
            "extra": {"stream": true, "speed": 9, "lang": "en"}}}),
        );
        let b = generic.body("a", "v1", 100);
        assert_eq!(b["response_format"], "pcm");
        assert_eq!(b["speed"], json!(9), "extra is merged last");
        assert_eq!(b["lang"], "en");
        assert!(b.get("sample_rate").is_none() && b.get("instructions").is_none());
        let server = preset(
            "chatterbox-server",
            json!({"voice": "Emily.wav", "options": {"response_format": "pcm"}}),
        );
        assert_eq!(server.body("a", "Emily.wav", 200)["response_format"], "wav");
    }

    /// Raw PCM that starts with sample -1 (FF FF, an MP3 frame sync) is
    /// speech when `pcm` was asked for.
    #[test]
    fn requested_pcm_starting_with_minus_one_is_audio() {
        let quiet = HttpReply {
            status: 200,
            content_type: Some("application/octet-stream".into()),
            retry_after: None,
            body: vec![0xFF, 0xFF, 0x00, 0x00],
        };
        let pcm = preset("generic", json!({"options": {"response_format": "pcm"}}));
        assert!(pcm.raw_pcm());
        assert_eq!(pcm.audio(&quiet, "T").unwrap().samples, vec![-1, 0]);
        assert!(!preset("generic", json!({})).raw_pcm(), "wav was asked for");
    }

    #[test]
    fn the_key_goes_in_a_bearer_header_only_when_allowed() {
        let key = Secret::new("sk-test");
        let local = preset("generic", json!({}));
        let r = local.synth_request("a", "v", 200, Some(&key));
        assert_eq!(r.url, "http://127.0.0.1:9/v1/audio/speech");
        assert_eq!(r.header_value("authorization"), Some("Bearer sk-test"));
        assert_eq!(
            local
                .synth_request("a", "v", 200, None)
                .header_value("authorization"),
            None
        );
        let lan = adapter(json!({"id": "p", "kind": "openai-compatible",
            "url": "http://10.1.2.3:8000/v1", "key_ref": "none",
            "options": {"allow_http": true}}));
        assert_eq!(
            lan.synth_request("a", "v", 200, Some(&key))
                .header_value("authorization"),
            None,
            "never over http to another computer"
        );
    }

    #[test]
    fn voice_sources_per_preset() {
        let url = |a: &OpenAi| match a.voices(None) {
            VoiceSource::Fetch {
                request,
                empty_on_error,
            } => (request.url, empty_on_error),
            VoiceSource::Fixed(v) => (format!("fixed {}", v.len()), false),
        };
        // No voice list in code (#235): OpenAI and openedai-speech have no
        // list API, so the voice is typed in.
        let o = adapter(
            json!({"id": "o", "kind": "openai-compatible", "options": {"preset": "openai"}}),
        );
        assert_eq!(url(&o).0, "fixed 0");
        assert_eq!(url(&preset("openedai-speech", json!({}))).0, "fixed 0");
        assert_eq!(
            url(&preset("kokoro-fastapi", json!({}))).0,
            "http://127.0.0.1:9/v1/audio/voices"
        );
        assert_eq!(
            url(&preset("localai", json!({"model": "a b"}))).0,
            "http://127.0.0.1:9/v1/audio/voices?model=a%20b"
        );
        assert_eq!(
            url(&preset("chatterbox-api", json!({}))).0,
            "http://127.0.0.1:9/voices"
        );
        assert_eq!(
            url(&preset("chatterbox-server", json!({"voice": "E.wav"}))).0,
            "http://127.0.0.1:9/get_predefined_voices"
        );
        assert_eq!(
            url(&preset(
                "generic",
                json!({"options": {"voices_path": "/v/list"}})
            )),
            ("http://127.0.0.1:9/v1/v/list".into(), true)
        );
    }

    #[test]
    fn models_come_from_the_models_list() {
        let o = adapter(json!({"id": "o", "kind": "openai-compatible",
            "options": {"preset": "openai"}}));
        let src = o.models(Some(&Secret::new("sk-1"))).unwrap();
        assert_eq!(src.request.url, "https://api.openai.com/v1/models");
        assert_eq!(
            src.request.header_value("authorization"),
            Some("Bearer sk-1")
        );
        assert!(!src.empty_on_error);
        let list = br#"{"object": "list", "data": [{"id": "chat-x"}, {"id": "a-tts"},
            {"id": "b-TTS-hd"}, {"id": "a-tts"}]}"#;
        let ids = |a: &OpenAi, b: &[u8]| {
            a.parse_models(b)
                .unwrap()
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(&o, list),
            vec!["a-tts", "b-TTS-hd"],
            "speech models only"
        );
        // A server: its text-to-speech models when it marks them, else all.
        let s = preset("speaches", json!({}));
        assert!(s.models(None).unwrap().empty_on_error);
        assert_eq!(
            ids(
                &s,
                br#"{"data": [{"id": "stt-1", "task": "automatic-speech-recognition"},
                {"id": "voice-1", "task": "text-to-speech"}]}"#
            ),
            vec!["voice-1"]
        );
        assert_eq!(
            ids(&s, br#"{"data": [{"id": "x"}, {"id": "y"}]}"#),
            vec!["x", "y"]
        );
        assert_eq!(
            o.parse_models(b"<html>").unwrap_err().reason,
            Reason::Format
        );
    }

    #[test]
    fn voice_list_shapes() {
        let ids = |v: Value| {
            parse_voice_list(&v)
                .into_iter()
                .map(|v| v.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(json!({"voices": ["af_heart", "am_adam"]})),
            vec!["af_heart", "am_adam"]
        );
        assert_eq!(
            ids(json!({"voices": [{"id": "a", "name": "A"}]})),
            vec!["a"]
        );
        assert_eq!(
            ids(
                json!({"data": [{"model": "kokoro", "voices": [{"name": "x", "language": "en"}]}]})
            ),
            vec!["x"]
        );
        assert_eq!(
            ids(json!([{"display_name": "Emily", "filename": "Emily.wav"}])),
            vec!["Emily.wav"]
        );
        let v =
            parse_voice_list(&json!({"voices": [{"name": "v2", "aliases": [], "language": "en"}]}));
        assert_eq!(
            v[0],
            VoiceInfo {
                id: "v2".into(),
                name: "v2".into(),
                language: "en".into()
            }
        );
        assert!(ids(json!({"nothing": 1})).is_empty());
    }
}
