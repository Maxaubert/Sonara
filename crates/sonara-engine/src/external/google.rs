//! Kind `google` (spec 5.4, 13.1, 13.2): Google Cloud Text-to-Speech, `POST
//! {url}/v1/text:synthesize` with `audioEncoding: PCM` (headerless 16-bit
//! mono; `LINEAR16` would add a WAV header, which is read too). The reply is
//! JSON with the audio in base64 (`audioContent`). The key goes in the
//! `X-goog-api-key` header, never in the URL. Input is limited in UTF-8
//! bytes (5000).
use super::adapter::{
    encode, key_allowed, Adapter, ErrorBody, HttpReply, HttpRequest, VoiceInfo, VoiceSource,
};
use super::audio::decode_body;
use super::error::{clean, headline, ExtError};
use super::keys::Secret;
use super::profile::{voice_locale, Kind, Profile};
use super::rate;
use super::split::Limit;
use crate::{PcmChunk, Reason};
use base64::Engine as _;
use serde_json::{json, Map, Value};

pub struct Google {
    base: String,
    language_code: Option<String>,
    sample_rate: u32,
    user_project: Option<String>,
    model_name: Option<String>,
    label: String,
}

impl Google {
    /// From a validated profile of kind `google`.
    pub fn new(p: &Profile) -> Google {
        Google {
            base: p.base_url().unwrap_or_default(),
            language_code: p.option_str("language_code").map(str::to_string),
            sample_rate: p
                .option_u64("sample_rate")
                .map(|r| r as u32)
                .unwrap_or(24_000),
            user_project: p.option_str("user_project").map(str::to_string),
            model_name: p.option_str("model_name").map(str::to_string),
            label: p.display_label(),
        }
    }

    fn with_key(&self, mut req: HttpRequest, key: Option<&Secret>) -> HttpRequest {
        if let Some(k) = key.filter(|_| key_allowed(&req.url)) {
            req = req.header("X-goog-api-key", k.expose());
        }
        if let Some(p) = &self.user_project {
            req = req.header("x-goog-user-project", p);
        }
        req
    }

    /// The request body (tests check it).
    pub fn body(&self, text: &str, voice: &str, wpm: u32) -> Value {
        let language = self
            .language_code
            .clone()
            .or_else(|| voice_locale(voice))
            .unwrap_or_else(|| "en-US".into());
        let mut v = Map::new();
        v.insert("languageCode".into(), json!(language));
        v.insert("name".into(), json!(voice));
        if let Some(m) = &self.model_name {
            v.insert("modelName".into(), json!(m));
        }
        let mut audio = Map::new();
        audio.insert("audioEncoding".into(), json!("PCM"));
        audio.insert("sampleRateHertz".into(), json!(self.sample_rate));
        if let Some(s) = rate::speed(Kind::Google, wpm) {
            audio.insert("speakingRate".into(), json!(s));
        }
        json!({"input": {"text": text}, "voice": v, "audioConfig": audio})
    }
}

impl Adapter for Google {
    fn input_limit(&self) -> Limit {
        Limit::Bytes(5000)
    }

    fn synth_request(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
    ) -> HttpRequest {
        self.with_key(
            HttpRequest::post_json(
                format!("{}/v1/text:synthesize", self.base),
                &self.body(text, voice, wpm),
            ),
            key,
        )
    }

    fn requested_rate(&self) -> Option<u32> {
        Some(self.sample_rate)
    }

    /// `{"audioContent": "<base64>"}` to PCM.
    fn audio(&self, reply: &HttpReply, label: &str) -> Result<PcmChunk, ExtError> {
        let format = |why: &str| ExtError::new(Reason::Format, format!("{label} {why}"));
        let v: Value = serde_json::from_slice(&reply.body)
            .map_err(|_| format("sent an answer that is not JSON"))?;
        let b64 = v
            .get("audioContent")
            .and_then(Value::as_str)
            .ok_or_else(|| format("sent no audioContent"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|_| format("sent audioContent that is not base64"))?;
        decode_body(&bytes, None, Some(self.sample_rate), label)
    }

    fn map_error(&self, reply: &HttpReply, _voice: &str, _listed: Option<bool>) -> ExtError {
        let s = reply.status;
        let eb = ErrorBody::parse(&reply.body);
        let raw = String::from_utf8_lossy(&reply.body);
        let message = eb
            .message
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let reason = match s {
            400 if raw.contains("API_KEY_INVALID") => Reason::Auth,
            401 | 403 => Reason::Auth,
            400 if eb.mentions("voice") => Reason::BadVoice,
            429 if message.contains("per day") || message.contains("billing") => Reason::Quota,
            429 => Reason::RateLimited,
            500..=599 => Reason::Server,
            _ => Reason::BadConfig,
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
        let mut url = format!("{}/v1/voices", self.base);
        if let Some(l) = &self.language_code {
            url.push_str(&format!("?languageCode={}", encode(l)));
        }
        VoiceSource::Fetch {
            request: self.with_key(HttpRequest::get(url), key),
            empty_on_error: false,
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
        Ok(v.get("voices")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| {
                let id = e.get("name").and_then(Value::as_str)?.to_string();
                let language = e
                    .get("languageCodes")
                    .and_then(Value::as_array)
                    .and_then(|l| l.first())
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Some(VoiceInfo {
                    name: id.clone(),
                    id,
                    language,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(options: Value) -> Google {
        Google::new(
            &Profile::from_json(&json!({"id": "g", "kind": "google",
                "voice": "en-US-Chirp3-HD-Kore", "options": options}))
            .unwrap(),
        )
    }

    fn reply(status: u16, body: &str) -> HttpReply {
        HttpReply {
            status,
            content_type: Some("application/json".into()),
            retry_after: None,
            body: body.as_bytes().to_vec(),
        }
    }

    fn google_error(code: u16, status: &str, message: &str, reason: Option<&str>) -> String {
        let details = match reason {
            Some(r) => json!([{"@type": "type.googleapis.com/google.rpc.ErrorInfo",
                "reason": r, "domain": "googleapis.com"}]),
            None => json!([]),
        };
        json!({"error": {"code": code, "message": message, "status": status,
            "details": details}})
        .to_string()
    }

    #[test]
    fn request_golden() {
        let a = adapter(json!({}));
        let r = a.synth_request(
            "Hello.",
            "en-US-Chirp3-HD-Kore",
            250,
            Some(&Secret::new("AIza-k")),
        );
        assert_eq!(
            r.url,
            "https://texttospeech.googleapis.com/v1/text:synthesize"
        );
        assert!(!r.url.contains("key="), "the key never goes in the URL");
        assert_eq!(r.header_value("x-goog-api-key"), Some("AIza-k"));
        assert_eq!(r.header_value("x-goog-user-project"), None);
        let body: Value = serde_json::from_slice(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"input": {"text": "Hello."},
                "voice": {"languageCode": "en-US", "name": "en-US-Chirp3-HD-Kore"},
                "audioConfig": {"audioEncoding": "PCM", "sampleRateHertz": 24000,
                    "speakingRate": 1.25}})
        );
        let g = adapter(json!({"language_code": "en-GB", "sample_rate": 16000,
            "user_project": "proj-1", "model_name": "gemini-2.5-flash-tts"}));
        let r = g.synth_request("Hi.", "Kore", 400, None);
        assert_eq!(r.header_value("x-goog-user-project"), Some("proj-1"));
        assert_eq!(r.header_value("x-goog-api-key"), None);
        assert_eq!(
            g.body("Hi.", "Kore", 400),
            json!({"input": {"text": "Hi."},
                "voice": {"languageCode": "en-GB", "name": "Kore",
                    "modelName": "gemini-2.5-flash-tts"},
                "audioConfig": {"audioEncoding": "PCM", "sampleRateHertz": 16000,
                    "speakingRate": 2.0}})
        );
        assert_eq!(g.requested_rate(), Some(16_000));
        // A voice without a locale prefix and no option: en-US.
        assert_eq!(
            adapter(json!({})).body("x", "Kore", 200)["voice"]["languageCode"],
            "en-US"
        );
        assert_eq!(a.input_limit(), Limit::Bytes(5000));
    }

    #[test]
    fn audio_content_is_base64_pcm() {
        let a = adapter(json!({}));
        let pcm: Vec<u8> = [100i16, -200, 300]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm);
        let got = a
            .audio(
                &reply(200, &json!({"audioContent": b64}).to_string()),
                "Google",
            )
            .unwrap();
        assert_eq!(got.samples, vec![100, -200, 300]);
        assert_eq!(got.sample_rate, 24_000);
        // LINEAR16 (a WAV header) is read too.
        let wav = crate::wav::encode(&PcmChunk {
            samples: vec![7, 8],
            sample_rate: 22_050,
            channels: 1,
        });
        let b64 = base64::engine::general_purpose::STANDARD.encode(&wav);
        let got = a
            .audio(
                &reply(200, &json!({"audioContent": b64}).to_string()),
                "Google",
            )
            .unwrap();
        assert_eq!((got.samples, got.sample_rate), (vec![7, 8], 22_050));
        for bad in [
            r#"{"other": 1}"#,
            r#"{"audioContent": "@@@"}"#,
            "RIFF",
            r#"{"audioContent": ""}"#,
        ] {
            assert_eq!(
                a.audio(&reply(200, bad), "Google").unwrap_err().reason,
                Reason::Format,
                "{bad}"
            );
        }
    }

    #[test]
    fn error_mapping_follows_the_table() {
        let a = adapter(json!({}));
        let err = |s, b: String| a.map_error(&reply(s, &b), "v", None).reason;
        assert_eq!(
            err(
                400,
                google_error(
                    400,
                    "INVALID_ARGUMENT",
                    "API key not valid. Please pass a valid API key.",
                    Some("API_KEY_INVALID")
                )
            ),
            Reason::Auth
        );
        assert_eq!(
            err(
                400,
                google_error(
                    400,
                    "INVALID_ARGUMENT",
                    "Voice 'en-US-Nobody' does not exist. Is it misspelled?",
                    None
                )
            ),
            Reason::BadVoice
        );
        assert_eq!(
            err(
                400,
                google_error(
                    400,
                    "INVALID_ARGUMENT",
                    "Sample rate 7 is not supported.",
                    None
                )
            ),
            Reason::BadConfig
        );
        assert_eq!(
            err(
                401,
                google_error(
                    401,
                    "UNAUTHENTICATED",
                    "Request had invalid authentication credentials.",
                    None
                )
            ),
            Reason::Auth
        );
        assert_eq!(
            err(403, google_error(403, "PERMISSION_DENIED", "Cloud Text-to-Speech API has not been used in project 1 before or it is disabled.", None)),
            Reason::Auth
        );
        assert_eq!(
            err(
                429,
                google_error(
                    429,
                    "RESOURCE_EXHAUSTED",
                    "Quota exceeded for quota metric 'Characters' and limit 'Characters per day'.",
                    None
                )
            ),
            Reason::Quota
        );
        assert_eq!(
            err(
                429,
                google_error(
                    429,
                    "RESOURCE_EXHAUSTED",
                    "Quota exceeded for quota metric 'Requests' and limit 'Requests per minute'.",
                    None
                )
            ),
            Reason::RateLimited
        );
        assert_eq!(
            err(500, google_error(500, "INTERNAL", "x", None)),
            Reason::Server
        );
        assert_eq!(
            err(503, google_error(503, "UNAVAILABLE", "x", None)),
            Reason::Server
        );
        let e = a.map_error(
            &reply(403, &google_error(403, "PERMISSION_DENIED", "Enable it at https://console.developers.google.com/apis/api/texttospeech.googleapis.com", None)),
            "v",
            None,
        );
        assert!(
            e.message
                .starts_with("Google Text-to-Speech refused the key (403): Enable it at"),
            "{}",
            e.message
        );
    }

    #[test]
    fn voice_list() {
        let a = adapter(json!({}));
        match a.voices(Some(&Secret::new("k"))) {
            VoiceSource::Fetch { request, .. } => {
                assert_eq!(request.url, "https://texttospeech.googleapis.com/v1/voices");
                assert_eq!(request.header_value("x-goog-api-key"), Some("k"));
            }
            VoiceSource::Fixed(_) => panic!("fetched"),
        }
        match adapter(json!({"language_code": "de-DE"})).voices(None) {
            VoiceSource::Fetch { request, .. } => assert_eq!(
                request.url,
                "https://texttospeech.googleapis.com/v1/voices?languageCode=de-DE"
            ),
            VoiceSource::Fixed(_) => panic!("fetched"),
        }
        let list = a
            .parse_voices(
                br#"{"voices": [{"languageCodes": ["en-US"], "name": "en-US-Chirp3-HD-Kore",
                    "ssmlGender": "FEMALE", "naturalSampleRateHertz": 24000},
                    {"languageCodes": [], "name": "x"}, {"languageCodes": ["de-DE"]}]}"#,
            )
            .unwrap();
        assert_eq!(
            list,
            vec![
                VoiceInfo {
                    id: "en-US-Chirp3-HD-Kore".into(),
                    name: "en-US-Chirp3-HD-Kore".into(),
                    language: "en-US".into()
                },
                VoiceInfo {
                    id: "x".into(),
                    name: "x".into(),
                    language: String::new()
                },
            ]
        );
    }
}
