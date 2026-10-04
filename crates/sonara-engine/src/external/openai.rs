//! Kind `openai-compatible` (spec 5.4, 13.1, 13.2): `POST {url}/audio/speech`
//! for OpenAI and the local servers that copy its API (Kokoro-FastAPI,
//! LocalAI, Speaches, openedai-speech, the two Chatterbox servers). WAV is
//! asked for by default (D8); the body is sniffed anyway.
use super::adapter::{
    key_allowed, Adapter, ErrorBody, HttpReply, HttpRequest, VoiceInfo, VoiceSource,
};
use super::error::{clean, headline, ExtError};
use super::keys::Secret;
use super::profile::{Kind, Preset, Profile};
use super::rate;
use super::split::Limit;
use crate::Reason;
use serde_json::{json, Map, Value};

/// OpenAI's voices for `gpt-4o-mini-tts` (spec 13.1).
pub const OPENAI_VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse",
    "marin", "cedar",
];
/// The newer voices `tts-1` and `tts-1-hd` lack.
const NOT_ON_TTS1: &[&str] = &["ballad", "verse", "marin", "cedar"];
/// openedai-speech's fixed list.
pub const OPENEDAI_VOICES: &[&str] = &["alloy", "echo", "fable", "onyx", "nova", "shimmer"];

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
    model: String,
    response_format: &'static str,
    sample_rate: Option<u32>,
    instructions: Option<String>,
    extra: Map<String, Value>,
    voices_path: Option<String>,
    label: String,
}

/// Percent-encode a query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
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
            model: p.effective_model().unwrap_or_default(),
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
        }
    }

    fn with_key(&self, mut req: HttpRequest, key: Option<&Secret>) -> HttpRequest {
        if let Some(k) = key.filter(|_| key_allowed(&req.url)) {
            req = req.header("Authorization", &format!("Bearer {}", k.expose()));
        }
        req
    }

    /// Whether `instructions` goes in the body: OpenAI takes it only for
    /// `gpt-4o-mini-tts` (tts-1 refuses it), and Kokoro-FastAPI and the
    /// Chatterbox servers do not take it; LocalAI forwards it to its
    /// expressive backends, so any other server gets it when it is set.
    fn takes_instructions(&self) -> bool {
        match self.preset {
            Preset::OpenAi => self.model.starts_with("gpt-4o-mini-tts"),
            Preset::KokoroFastApi | Preset::ChatterboxApi | Preset::ChatterboxServer => false,
            Preset::LocalAi | Preset::Speaches | Preset::OpenedAiSpeech | Preset::Generic => true,
        }
    }

    /// The request body (tests check it per preset).
    pub fn body(&self, text: &str, voice: &str, wpm: u32) -> Value {
        let mut b = Map::new();
        b.insert("model".into(), json!(self.model));
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

    fn fixed(ids: &[&str]) -> Vec<VoiceInfo> {
        ids.iter().map(|v| VoiceInfo::named(v)).collect()
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

    fn requested_rate(&self) -> Option<u32> {
        Some(self.sample_rate.unwrap_or(24_000))
    }

    fn map_error(&self, reply: &HttpReply, _voice: &str) -> ExtError {
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
        let mut e = ExtError::new(
            reason,
            format!(
                "{} ({s}): {}",
                headline(reason, &self.label, ""),
                eb.text_or(s)
            ),
        )
        .with_status(s);
        if matches!(s, 429 | 503) {
            e.retry_after = reply.retry_after;
        }
        e
    }

    fn voices(&self, key: Option<&Secret>) -> VoiceSource {
        let get = |url: String| self.with_key(HttpRequest::get(url), key);
        let fetch = |url: String| VoiceSource::Fetch {
            request: get(url),
            empty_on_error: false,
        };
        match self.preset {
            Preset::OpenAi => {
                let old = self.model == "tts-1" || self.model == "tts-1-hd";
                let ids: Vec<&str> = OPENAI_VOICES
                    .iter()
                    .copied()
                    .filter(|v| !old || !NOT_ON_TTS1.contains(v))
                    .collect();
                VoiceSource::Fixed(Self::fixed(&ids))
            }
            Preset::OpenedAiSpeech => VoiceSource::Fixed(Self::fixed(OPENEDAI_VOICES)),
            Preset::KokoroFastApi | Preset::Speaches => {
                fetch(format!("{}/audio/voices", self.base))
            }
            Preset::LocalAi => fetch(format!(
                "{}/audio/voices?model={}",
                self.base,
                encode(&self.model)
            )),
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
/// strings or objects (`id`, `voice_id`, `filename` or `name`).
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
                    .or_else(|| s(e, "filename"))
                    .or_else(|| s(e, "name"))?;
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
        let err = |s, b: &str| a.map_error(&reply(s, b), "v").reason;
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
            "marin",
        );
        assert_eq!(e.status, Some(401));
        assert_eq!(
            e.message,
            "OpenAI refused the key (401): Incorrect API key provided: [redacted]"
        );
        let mut r = reply(429, r#"{"error": {"code": "slow_down"}}"#);
        r.retry_after = Some(std::time::Duration::from_secs(1));
        assert_eq!(
            a.map_error(&r, "v").retry_after,
            Some(std::time::Duration::from_secs(1))
        );
    }

    #[test]
    fn request_bodies_per_preset() {
        let openai = adapter(json!({"id": "o", "kind": "openai-compatible",
            "options": {"preset": "openai", "instructions": "Calm."}}));
        assert_eq!(
            openai.body("Hi.", "marin", 250),
            json!({"model": "gpt-4o-mini-tts", "input": "Hi.", "voice": "marin",
                "response_format": "wav", "speed": 1.25, "instructions": "Calm."})
        );
        assert_eq!(
            openai.body("Hi.", "voice_abc123", 200)["voice"],
            json!({"id": "voice_abc123"})
        );
        let tts1 = adapter(
            json!({"id": "o", "kind": "openai-compatible", "model": "tts-1",
            "options": {"preset": "openai", "instructions": "Calm."}}),
        );
        assert!(tts1.body("Hi.", "alloy", 200).get("instructions").is_none());
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
        assert_eq!(kokoro.body("a", "af_heart", 200)["stream"], json!(false));
        let speaches = preset("speaches", json!({}));
        assert_eq!(
            speaches.body("a", "af_heart", 200)["sample_rate"],
            json!(24_000)
        );
        let speaches = preset("speaches", json!({"options": {"sample_rate": 16000}}));
        assert_eq!(speaches.body("a", "x", 200)["sample_rate"], json!(16_000));
        let generic = preset(
            "generic",
            json!({"options": {"response_format": "pcm",
            "extra": {"stream": true, "speed": 9, "lang": "en"}}}),
        );
        let b = generic.body("a", "alloy", 100);
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
        let o = adapter(
            json!({"id": "o", "kind": "openai-compatible", "options": {"preset": "openai"}}),
        );
        assert_eq!(url(&o).0, "fixed 13");
        let t1 = adapter(
            json!({"id": "o", "kind": "openai-compatible", "model": "tts-1-hd",
            "options": {"preset": "openai"}}),
        );
        assert_eq!(url(&t1).0, "fixed 9");
        assert_eq!(url(&preset("openedai-speech", json!({}))).0, "fixed 6");
        assert_eq!(
            url(&preset("kokoro-fastapi", json!({}))).0,
            "http://127.0.0.1:9/v1/audio/voices"
        );
        assert_eq!(
            url(&preset("localai", json!({"model": "kokoro v1"}))).0,
            "http://127.0.0.1:9/v1/audio/voices?model=kokoro%20v1"
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
        let v = parse_voice_list(
            &json!({"voices": [{"name": "alloy", "aliases": [], "language": "en"}]}),
        );
        assert_eq!(
            v[0],
            VoiceInfo {
                id: "alloy".into(),
                name: "alloy".into(),
                language: "en".into()
            }
        );
        assert!(ids(json!({"nothing": 1})).is_empty());
    }
}
