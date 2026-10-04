//! Kind `elevenlabs` (spec 5.4, 13.1, 13.2): `POST {url}/v1/text-to-speech/
//! {voice_id}?output_format=pcm_24000`, the whole body (not the stream
//! endpoint), raw 16-bit mono PCM at the named rate. The key goes in
//! `xi-api-key`. The voice list is paged (`/v2/voices`, `next_page_token`).
use super::adapter::{
    encode, key_allowed, Adapter, ErrorBody, HttpReply, HttpRequest, VoiceInfo, VoiceSource,
};
use super::error::{clean, headline, ExtError};
use super::keys::Secret;
use super::profile::{Kind, Profile};
use super::rate;
use super::split::Limit;
use crate::Reason;
use serde_json::{json, Map, Value};

/// The default output format.
pub const DEFAULT_FORMAT: &str = "pcm_24000";
/// Voices asked for per page.
const PAGE_SIZE: u32 = 100;

/// 403 codes about the plan or the model, not the key.
const PLAN_CODES: &[&str] = &[
    "feature_not_available",
    "subscription_required",
    "model_access_denied",
];

pub struct ElevenLabs {
    base: String,
    model: String,
    output_format: String,
    settings: Map<String, Value>,
    language_code: Option<String>,
    enable_logging: bool,
    label: String,
}

impl ElevenLabs {
    /// From a validated profile of kind `elevenlabs`.
    pub fn new(p: &Profile) -> ElevenLabs {
        let mut settings = Map::new();
        for k in ["stability", "similarity_boost", "style"] {
            if let Some(v) = p.options.get(k) {
                settings.insert(k.into(), v.clone());
            }
        }
        ElevenLabs {
            base: p.base_url().unwrap_or_default(),
            model: p.effective_model().unwrap_or_default(),
            output_format: p
                .option_str("output_format")
                .unwrap_or(DEFAULT_FORMAT)
                .to_string(),
            settings,
            language_code: p.option_str("language_code").map(str::to_string),
            enable_logging: p
                .options
                .get("enable_logging")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            label: p.display_label(),
        }
    }

    fn with_key(&self, mut req: HttpRequest, key: Option<&Secret>) -> HttpRequest {
        if let Some(k) = key.filter(|_| key_allowed(&req.url)) {
            req = req.header("xi-api-key", k.expose());
        }
        req
    }

    /// The request body (tests check it). `voice_settings` replaces the
    /// voice's stored settings for the request, so it is left out when no
    /// setting is set and the speed is 1.0.
    pub fn body(&self, text: &str, wpm: u32) -> Value {
        let mut settings = self.settings.clone();
        if let Some(s) = rate::speed(Kind::ElevenLabs, wpm) {
            if !settings.is_empty() || (s - 1.0).abs() > f64::EPSILON {
                settings.insert("speed".into(), json!(s));
            }
        }
        let mut b = Map::new();
        b.insert("text".into(), json!(text));
        b.insert("model_id".into(), json!(self.model));
        if !settings.is_empty() {
            b.insert("voice_settings".into(), Value::Object(settings));
        }
        if let Some(l) = &self.language_code {
            b.insert("language_code".into(), json!(l));
        }
        Value::Object(b)
    }

    fn voices_url(&self, token: Option<&str>) -> String {
        let mut url = format!("{}/v2/voices?page_size={PAGE_SIZE}", self.base);
        if let Some(t) = token {
            url.push_str(&format!("&next_page_token={}", encode(t)));
        }
        url
    }
}

impl Adapter for ElevenLabs {
    fn input_limit(&self) -> Limit {
        // The smallest per-model limit (v3); a sentence never comes near.
        Limit::Chars(5000)
    }

    fn synth_request(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
    ) -> HttpRequest {
        let mut url = format!(
            "{}/v1/text-to-speech/{}?output_format={}",
            self.base,
            encode(voice),
            self.output_format
        );
        if !self.enable_logging {
            url.push_str("&enable_logging=false");
        }
        self.with_key(HttpRequest::post_json(url, &self.body(text, wpm)), key)
    }

    fn requested_rate(&self) -> Option<u32> {
        self.output_format
            .strip_prefix("pcm_")
            .and_then(|r| r.parse().ok())
    }

    fn raw_pcm(&self) -> bool {
        self.output_format.starts_with("pcm_")
    }

    fn map_error(&self, reply: &HttpReply, _voice: &str, _listed: Option<bool>) -> ExtError {
        let s = reply.status;
        let eb = ErrorBody::parse(&reply.body);
        let codes: Vec<&str> = [&eb.code, &eb.status, &eb.kind]
            .into_iter()
            .filter_map(|c| c.as_deref())
            .collect();
        let has = |c: &str| codes.contains(&c);
        let plan = PLAN_CODES.iter().any(|c| has(c));
        let reason = match s {
            401 if has("quota_exceeded") => Reason::Quota,
            401 => Reason::Auth,
            402 => Reason::Quota,
            403 if has("voice_access_denied") => Reason::BadVoice,
            403 if plan => Reason::BadConfig,
            403 => Reason::Auth,
            404 | 400 if has("voice_not_found") || has("invalid_voice_id") => Reason::BadVoice,
            404 if !has("model_not_found") && eb.mentions("voice") => Reason::BadVoice,
            429 => Reason::RateLimited,
            500..=599 => Reason::Server,
            _ => Reason::BadConfig,
        };
        let mut message = format!(
            "{} ({s}): {}",
            headline(reason, &self.label, ""),
            eb.text_or(s)
        );
        if plan && self.output_format != DEFAULT_FORMAT {
            message.push_str(&format!(
                "; output_format {} may need a higher plan, try {DEFAULT_FORMAT}",
                self.output_format
            ));
        }
        let mut e = ExtError::new(reason, message).with_status(s);
        if matches!(s, 429 | 503) {
            e.retry_after = reply.retry_after;
        }
        e
    }

    fn voices(&self, key: Option<&Secret>) -> VoiceSource {
        VoiceSource::Fetch {
            request: self.with_key(HttpRequest::get(self.voices_url(None)), key),
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
        let s = |e: &Value, k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
        Ok(v.get("voices")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| {
                let id = s(e, "voice_id")?;
                Some(VoiceInfo {
                    name: s(e, "name").unwrap_or_else(|| id.clone()),
                    language: e
                        .get("labels")
                        .and_then(|l| s(l, "language"))
                        .unwrap_or_default(),
                    id,
                })
            })
            .collect())
    }

    fn next_voices_page(&self, body: &[u8], key: Option<&Secret>) -> Option<HttpRequest> {
        let v: Value = serde_json::from_slice(body).ok()?;
        if v.get("has_more").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let token = v
            .get("next_page_token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())?;
        Some(self.with_key(HttpRequest::get(self.voices_url(Some(token))), key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(options: Value) -> ElevenLabs {
        ElevenLabs::new(
            &Profile::from_json(&json!({"id": "el", "kind": "elevenlabs",
                "voice": "JBFqnCBsd6RMkjVDRZzb", "options": options}))
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

    /// The current error shape: `detail` with `code` and `status`.
    fn detail(code: &str) -> String {
        format!(
            r#"{{"detail": {{"type": "t", "code": "{code}", "message": "m {code}",
                "status": "{code}", "request_id": "r", "param": null}}}}"#
        )
    }

    #[test]
    fn error_mapping_follows_the_table() {
        let a = adapter(json!({}));
        let err = |s, b: &str| a.map_error(&reply(s, b), "v", None).reason;
        for (status, code, want) in [
            (401, "invalid_api_key", Reason::Auth),
            (401, "missing_api_key", Reason::Auth),
            (402, "insufficient_credits", Reason::Quota),
            (403, "voice_access_denied", Reason::BadVoice),
            (403, "feature_not_available", Reason::BadConfig),
            (403, "subscription_required", Reason::BadConfig),
            (403, "model_access_denied", Reason::BadConfig),
            (404, "voice_not_found", Reason::BadVoice),
            (400, "invalid_voice_id", Reason::BadVoice),
            (404, "model_not_found", Reason::BadConfig),
            (400, "unsupported_model", Reason::BadConfig),
            (400, "invalid_output_format", Reason::BadConfig),
            (400, "invalid_voice_settings", Reason::BadConfig),
            (400, "text_too_long", Reason::BadConfig),
            (400, "empty_text", Reason::BadConfig),
            (400, "invalid_text", Reason::BadConfig),
            (429, "rate_limit_exceeded", Reason::RateLimited),
            (429, "concurrent_limit_exceeded", Reason::RateLimited),
            (429, "system_busy", Reason::RateLimited),
            (500, "internal_error", Reason::Server),
            (503, "service_unavailable", Reason::Server),
            (503, "maintenance", Reason::Server),
        ] {
            assert_eq!(err(status, &detail(code)), want, "{status} {code}");
        }
        // The older quota shape: 401 with status quota_exceeded.
        assert_eq!(
            err(
                401,
                r#"{"detail": {"status": "quota_exceeded", "message": "This request exceeds your quota."}}"#
            ),
            Reason::Quota
        );
        // A validation error (422 detail array) is a settings problem.
        assert_eq!(
            err(
                422,
                r#"{"detail": [{"loc": ["body", "text"], "msg": "field required"}]}"#
            ),
            Reason::BadConfig
        );
    }

    #[test]
    fn messages_name_the_cause_and_suggest_the_default_format() {
        let pro = adapter(json!({"output_format": "pcm_44100"}));
        let e = pro.map_error(&reply(403, &detail("subscription_required")), "v", None);
        assert_eq!(e.status, Some(403));
        assert_eq!(
            e.message,
            "ElevenLabs settings do not work (403): m subscription_required; \
             output_format pcm_44100 may need a higher plan, try pcm_24000"
        );
        let plain = adapter(json!({}));
        let e = plain.map_error(&reply(401, &detail("invalid_api_key")), "v", None);
        assert_eq!(
            e.message,
            "ElevenLabs refused the key (401): m invalid_api_key"
        );
    }

    #[test]
    fn request_golden() {
        let a = adapter(
            json!({"stability": 0.4, "similarity_boost": 0.8, "style": 0.1,
            "language_code": "de", "enable_logging": false, "output_format": "pcm_16000"}),
        );
        let key = Secret::new("xi-test-key");
        let r = a.synth_request("Hallo.", "JBFqnCBsd6RMkjVDRZzb", 250, Some(&key));
        assert_eq!(
            r.url,
            "https://api.elevenlabs.io/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb\
             ?output_format=pcm_16000&enable_logging=false"
        );
        assert_eq!(r.header_value("xi-api-key"), Some("xi-test-key"));
        assert_eq!(r.header_value("authorization"), None);
        let body: Value = serde_json::from_slice(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"text": "Hallo.", "model_id": "eleven_flash_v2_5", "language_code": "de",
                "voice_settings": {"stability": 0.4, "similarity_boost": 0.8, "style": 0.1,
                    "speed": 1.2}})
        );
        assert_eq!(a.requested_rate(), Some(16_000));
        let d = adapter(json!({}));
        let r = d.synth_request("Hi.", "a/b c", 140, None);
        assert_eq!(
            r.url,
            "https://api.elevenlabs.io/v1/text-to-speech/a%2Fb%20c?output_format=pcm_24000"
        );
        assert_eq!(r.header_value("xi-api-key"), None);
        assert_eq!(
            d.body("Hi.", 140),
            json!({"text": "Hi.", "model_id": "eleven_flash_v2_5",
                "voice_settings": {"speed": 0.7}})
        );
        assert_eq!(d.requested_rate(), Some(24_000));
    }

    /// `voice_settings` replaces the voice's stored settings for the
    /// request, so it is left out when there is nothing to change.
    #[test]
    fn voice_settings_left_out_at_the_default_rate() {
        assert_eq!(
            adapter(json!({})).body("Hi.", 200),
            json!({"text": "Hi.", "model_id": "eleven_flash_v2_5"})
        );
        assert_eq!(
            adapter(json!({"stability": 0.3})).body("Hi.", 200),
            json!({"text": "Hi.", "model_id": "eleven_flash_v2_5",
                "voice_settings": {"stability": 0.3, "speed": 1.0}})
        );
    }

    #[test]
    fn voice_pages() {
        let a = adapter(json!({}));
        let first = match a.voices(Some(&Secret::new("k"))) {
            VoiceSource::Fetch { request, .. } => request,
            VoiceSource::Fixed(_) => panic!("fetched"),
        };
        assert_eq!(
            first.url,
            "https://api.elevenlabs.io/v2/voices?page_size=100"
        );
        assert_eq!(first.header_value("xi-api-key"), Some("k"));
        let page = br#"{"voices": [
            {"voice_id": "a1", "name": "Alice", "labels": {"language": "en", "accent": "british"}},
            {"voice_id": "b2", "name": "Bob", "labels": {}},
            {"name": "no id"}],
            "has_more": true, "next_page_token": "tok/2", "total_count": 5}"#;
        assert_eq!(
            a.parse_voices(page).unwrap(),
            vec![
                VoiceInfo {
                    id: "a1".into(),
                    name: "Alice".into(),
                    language: "en".into()
                },
                VoiceInfo {
                    id: "b2".into(),
                    name: "Bob".into(),
                    language: String::new()
                },
            ]
        );
        let next = a.next_voices_page(page, None).unwrap();
        assert_eq!(
            next.url,
            "https://api.elevenlabs.io/v2/voices?page_size=100&next_page_token=tok%2F2"
        );
        assert!(a
            .next_voices_page(br#"{"voices": [], "has_more": false}"#, None)
            .is_none());
        assert!(a
            .next_voices_page(br#"{"voices": [], "has_more": true}"#, None)
            .is_none());
    }
}
