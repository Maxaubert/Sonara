//! Kind `deepgram` (spec 5.4, 9, 13.1, 13.2): `POST {url}/v1/speak?model=
//! {voice}&encoding=linear16&container=none&sample_rate=24000` with
//! `{"text"}`, raw 16-bit mono PCM back. The voice is the model (Aura voices
//! such as `aura-2-thalia-en`). The key goes in `Authorization: Token`.
//!
//! Speed (uncertain, spec 13.3 fact 2): sent as the `speed` query parameter
//! except at 1.0. When Deepgram refuses it (a `bad_config` naming `speed`),
//! the same part is sent once more without it and the adapter stops sending
//! it (`adapt`), so a model without speed control still reads, at its own
//! pace.
use super::adapter::{encode, key_allowed};
use super::adapter::{Adapter, ErrorBody, HttpReply, HttpRequest, VoiceInfo, VoiceSource};
use super::error::{clean, headline, ExtError};
use super::keys::Secret;
use super::profile::{Kind, Profile};
use super::rate;
use super::split::Limit;
use crate::Reason;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Deepgram {
    base: String,
    sample_rate: u32,
    label: String,
    /// Deepgram refused `speed` once: it is not sent again.
    no_speed: AtomicBool,
}

impl Deepgram {
    /// From a validated profile of kind `deepgram`.
    pub fn new(p: &Profile) -> Deepgram {
        Deepgram {
            base: p.base_url().unwrap_or_default(),
            sample_rate: p
                .option_u64("sample_rate")
                .map(|r| r as u32)
                .unwrap_or(24_000),
            label: p.display_label(),
            no_speed: AtomicBool::new(false),
        }
    }

    fn with_key(&self, req: HttpRequest, key: Option<&Secret>) -> HttpRequest {
        match key.filter(|_| key_allowed(&req.url)) {
            Some(k) => req.header("Authorization", &format!("Token {}", k.expose())),
            None => req,
        }
    }

    /// Whether `speed` is still sent.
    pub fn sends_speed(&self) -> bool {
        !self.no_speed.load(Ordering::SeqCst)
    }

    /// The synthesis URL (tests check it).
    pub fn speak_url(&self, voice: &str, wpm: u32) -> String {
        let mut url = format!(
            "{}/v1/speak?model={}&encoding=linear16&container=none&sample_rate={}",
            self.base,
            encode(voice),
            self.sample_rate
        );
        if let Some(s) = rate::speed(Kind::Deepgram, wpm).filter(|_| self.sends_speed()) {
            url.push_str(&format!("&speed={s}"));
        }
        url
    }
}

impl Adapter for Deepgram {
    fn input_limit(&self) -> Limit {
        Limit::Chars(2000)
    }

    fn synth_request(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
    ) -> HttpRequest {
        self.with_key(
            HttpRequest::post_json(self.speak_url(voice, wpm), &json!({"text": text})),
            key,
        )
    }

    fn requested_rate(&self) -> Option<u32> {
        Some(self.sample_rate)
    }

    fn raw_pcm(&self) -> bool {
        true
    }

    fn map_error(&self, reply: &HttpReply, _voice: &str, _listed: Option<bool>) -> ExtError {
        let s = reply.status;
        let eb = ErrorBody::parse(&reply.body);
        let reason = match s {
            401 | 403 => Reason::Auth,
            // Not in the fetched docs (spec 13.3): by convention.
            402 => Reason::Quota,
            400 if eb.mentions("speed") => Reason::BadConfig,
            400 if eb.mentions("model") => Reason::BadVoice,
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

    /// A refused `speed`: send the part once more without it, and never
    /// again with it.
    fn adapt(&self, error: &ExtError) -> bool {
        error.reason == Reason::BadConfig
            && error.message.to_ascii_lowercase().contains("speed")
            && !self.no_speed.swap(true, Ordering::SeqCst)
    }

    fn voices(&self, key: Option<&Secret>) -> VoiceSource {
        VoiceSource::Fetch {
            request: self.with_key(HttpRequest::get(format!("{}/v1/models", self.base)), key),
            empty_on_error: false,
        }
    }

    /// The `tts` array of `/v1/models`.
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
        Ok(v.get("tts")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| {
                let id = s(e, "canonical_name")?;
                let name = match (s(e, "name"), s(e, "architecture")) {
                    (Some(n), Some(a)) => format!("{n} ({a})"),
                    (Some(n), None) => n,
                    _ => id.clone(),
                };
                let language = e
                    .get("languages")
                    .and_then(Value::as_array)
                    .and_then(|l| l.first())
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Some(VoiceInfo { id, name, language })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(options: Value) -> Deepgram {
        Deepgram::new(
            &Profile::from_json(&json!({"id": "dg", "kind": "deepgram",
                "voice": "aura-2-thalia-en", "options": options}))
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

    #[test]
    fn error_mapping_follows_the_table() {
        let a = adapter(json!({}));
        let err = |s, b: &str| a.map_error(&reply(s, b), "aura-2-thalia-en", None).reason;
        let body = |code: &str, msg: &str| {
            format!(r#"{{"err_code": "{code}", "err_msg": "{msg}", "request_id": "r"}}"#)
        };
        for (status, b, want) in [
            (
                401,
                body("INVALID_AUTH", "Invalid credentials."),
                Reason::Auth,
            ),
            (
                403,
                body("INSUFFICIENT_PERMISSIONS", "No access."),
                Reason::Auth,
            ),
            (
                402,
                body("ASR_PAYMENT_REQUIRED", "Out of credit."),
                Reason::Quota,
            ),
            (
                400,
                body("INVALID_QUERY_PARAMETER", "No such model: aura-9-x"),
                Reason::BadVoice,
            ),
            (
                400,
                body("INVALID_QUERY_PARAMETER", "speed is not supported"),
                Reason::BadConfig,
            ),
            (400, body("Bad Request", "Invalid JSON"), Reason::BadConfig),
            (
                413,
                body("PAYLOAD_TOO_LARGE", "Text too long"),
                Reason::BadConfig,
            ),
            (422, body("UNPROCESSABLE", "Empty text"), Reason::BadConfig),
            (
                429,
                body("TOO_MANY_REQUESTS", "Slow down"),
                Reason::RateLimited,
            ),
            (500, body("INTERNAL", "Oops"), Reason::Server),
            (503, String::new(), Reason::Server),
        ] {
            assert_eq!(err(status, &b), want, "{status} {b}");
        }
        // The other body shape: err_code, message, details.
        let e = a.map_error(
            &reply(
                401,
                r#"{"err_code": "INVALID_AUTH", "message": "Invalid credentials.", "details": "x"}"#,
            ),
            "v",
            None,
        );
        assert_eq!(
            e.message,
            "Deepgram refused the key (401): Invalid credentials."
        );
    }

    #[test]
    fn request_golden() {
        let a = adapter(json!({}));
        let key = Secret::new("dg-test-key-0123456789");
        let r = a.synth_request("Hello.", "aura-2-thalia-en", 250, Some(&key));
        assert_eq!(
            r.url,
            "https://api.deepgram.com/v1/speak?model=aura-2-thalia-en&encoding=linear16\
             &container=none&sample_rate=24000&speed=1.25"
        );
        assert_eq!(
            r.header_value("authorization"),
            Some("Token dg-test-key-0123456789")
        );
        let body: Value = serde_json::from_slice(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(body, json!({"text": "Hello."}));
        // No speed at 1.0; the rate option is sent and asked for.
        let b = adapter(json!({"sample_rate": 16000}));
        assert_eq!(
            b.speak_url("aura-asteria-en", 200),
            "https://api.deepgram.com/v1/speak?model=aura-asteria-en&encoding=linear16\
             &container=none&sample_rate=16000"
        );
        assert_eq!(b.requested_rate(), Some(16_000));
        assert!(b.raw_pcm());
        assert!(b.speak_url("v", 400).ends_with("&speed=1.5"));
        assert!(b.speak_url("v", 100).ends_with("&speed=0.7"));
    }

    #[test]
    fn a_refused_speed_is_dropped_once_and_remembered() {
        let a = adapter(json!({}));
        let refused = a.map_error(
            &reply(
                400,
                r#"{"err_code": "INVALID_QUERY_PARAMETER", "err_msg": "Unknown parameter: speed"}"#,
            ),
            "v",
            None,
        );
        assert_eq!(refused.reason, Reason::BadConfig);
        assert!(
            a.adapt(&refused),
            "first refusal: try once more without speed"
        );
        assert!(!a.sends_speed());
        assert!(!a.speak_url("v", 250).contains("speed"));
        assert!(!a.adapt(&refused), "only once");
        let other = ExtError::new(Reason::BadConfig, "Invalid JSON");
        assert!(!adapter(json!({})).adapt(&other));
        let auth = ExtError::new(Reason::Auth, "speed");
        assert!(!adapter(json!({})).adapt(&auth));
    }

    #[test]
    fn voices_come_from_the_tts_models() {
        let a = adapter(json!({}));
        let first = match a.voices(Some(&Secret::new("k"))) {
            VoiceSource::Fetch { request, .. } => request,
            VoiceSource::Fixed(_) => panic!("fetched"),
        };
        assert_eq!(first.url, "https://api.deepgram.com/v1/models");
        assert_eq!(first.header_value("authorization"), Some("Token k"));
        let body = br#"{"stt": [{"name": "nova-3", "canonical_name": "nova-3"}],
            "tts": [
              {"name": "thalia", "canonical_name": "aura-2-thalia-en",
               "architecture": "aura-2", "languages": ["en", "en-US"]},
              {"name": "asteria", "canonical_name": "aura-asteria-en"},
              {"name": "no canonical name"}]}"#;
        assert_eq!(
            a.parse_voices(body).unwrap(),
            vec![
                VoiceInfo {
                    id: "aura-2-thalia-en".into(),
                    name: "thalia (aura-2)".into(),
                    language: "en".into()
                },
                VoiceInfo {
                    id: "aura-asteria-en".into(),
                    name: "asteria".into(),
                    language: String::new()
                },
            ]
        );
    }
}
