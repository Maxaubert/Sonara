//! Kind `cartesia` (spec 5.4, 13.1, 13.2): `POST {url}/tts/bytes` with the
//! whole body (not the WebSocket API), raw 16-bit mono PCM at the rate asked
//! for. The key goes in `Authorization: Bearer`, the dated API version in
//! `Cartesia-Version` (`options.api_version`, so a version Cartesia retires
//! is a setting, not a new release). The voice list is paged by cursor.
//! No model id in code (#235): Cartesia needs one in every request and has
//! no model list API Sonara uses, so the user types it (the settings page
//! links Cartesia's model page); a profile without one says "choose a
//! model".
use super::adapter::{
    encode, key_allowed, Adapter, ErrorBody, HttpReply, HttpRequest, VoiceInfo, VoiceSource,
};
use super::error::{clean, headline, model_message, ExtError};
use super::keys::Secret;
use super::profile::{Kind, Profile};
use super::rate;
use super::split::Limit;
use crate::Reason;
use serde_json::{json, Map, Value};

/// Cartesia's API version (spec 5.4; the API pins its behaviour to the
/// version date, so it is the API's contract, not a model or a voice).
pub const CARTESIA_VERSION: &str = "2026-08-14";
/// Cartesia's raw PCM rates.
pub const CARTESIA_RATES: &[u64] = &[8_000, 16_000, 22_050, 24_000, 44_100, 48_000];

/// Voices asked for per page.
const PAGE_SIZE: u32 = 100;

pub struct Cartesia {
    base: String,
    /// `None`: the profile names none yet (`Profile::missing_model`).
    model: Option<String>,
    api_version: String,
    language: String,
    sample_rate: u32,
    label: String,
}

impl Cartesia {
    /// From a validated profile of kind `cartesia`.
    pub fn new(p: &Profile) -> Cartesia {
        Cartesia {
            base: p.base_url().unwrap_or_default(),
            model: p.model.clone(),
            api_version: p
                .option_str("api_version")
                .unwrap_or(CARTESIA_VERSION)
                .to_string(),
            language: p.option_str("language").unwrap_or("en").to_string(),
            sample_rate: p
                .option_u64("sample_rate")
                .map(|r| r as u32)
                .unwrap_or(24_000),
            label: p.display_label(),
        }
    }

    fn with_key(&self, mut req: HttpRequest, key: Option<&Secret>) -> HttpRequest {
        if let Some(k) = key.filter(|_| key_allowed(&req.url)) {
            req = req.header("Authorization", &format!("Bearer {}", k.expose()));
        }
        req.header("Cartesia-Version", &self.api_version)
    }

    /// The request body (tests check it). `generation_config` is left out
    /// at speed 1.0, so a model without it is not asked for it.
    pub fn body(&self, text: &str, voice: &str, wpm: u32) -> Value {
        let mut b = Map::new();
        if let Some(m) = &self.model {
            b.insert("model_id".into(), json!(m));
        }
        b.insert("transcript".into(), json!(text));
        b.insert("voice".into(), json!({"id": voice}));
        b.insert(
            "output_format".into(),
            json!({"container": "raw", "encoding": "pcm_s16le", "sample_rate": self.sample_rate}),
        );
        b.insert("language".into(), json!(self.language));
        if let Some(s) = rate::speed(Kind::Cartesia, wpm) {
            if (s - 1.0).abs() > f64::EPSILON {
                b.insert("generation_config".into(), json!({"speed": s}));
            }
        }
        Value::Object(b)
    }

    fn voices_url(&self, after: Option<&str>) -> String {
        let mut url = format!("{}/voices?limit={PAGE_SIZE}", self.base);
        if let Some(a) = after {
            url.push_str(&format!("&starting_after={}", encode(a)));
        }
        url
    }
}

impl Adapter for Cartesia {
    fn input_limit(&self) -> Limit {
        // No documented limit; Sonara's choice (spec 13.1).
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
            HttpRequest::post_json(
                format!("{}/tts/bytes", self.base),
                &self.body(text, voice, wpm),
            ),
            key,
        )
    }

    /// `/tts/bytes` sends its raw PCM as it is made (#235).
    fn bytes_request(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
    ) -> Option<HttpRequest> {
        Some(self.synth_request(text, voice, wpm, key))
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
        let code = eb.code.as_deref().unwrap_or_default();
        let reason = match s {
            401 => Reason::Auth,
            402 => Reason::Quota,
            403 if code == "plan_upgrade_required" => Reason::BadConfig,
            403 => Reason::Auth,
            _ if code == "quota_exceeded" => Reason::Quota,
            404 | 422 if code == "voice_not_found" || code == "voice_model_mismatch" => {
                Reason::BadVoice
            }
            404 if code.is_empty() && eb.mentions("voice") => Reason::BadVoice,
            429 => Reason::RateLimited,
            500..=599 => Reason::Server,
            _ => Reason::BadConfig,
        };
        let model = self.model.as_deref().unwrap_or_default();
        let message = if reason == Reason::BadConfig && code == "model_not_found" {
            model_message(&self.label, model, s, &eb.text_or(s))
        } else if reason == Reason::BadConfig && eb.mentions("version") {
            format!(
                "{} ({s}): {}; check options.api_version (now {}) and the model ({model})",
                headline(reason, &self.label, ""),
                eb.text_or(s),
                self.api_version
            )
        } else {
            format!(
                "{} ({s}): {}",
                headline(reason, &self.label, ""),
                eb.text_or(s)
            )
        };
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
        // `data` (paged API); an older plain array is read too.
        let items = v
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| v.as_array())
            .map(|a| a.as_slice())
            .unwrap_or_default();
        Ok(items
            .iter()
            .filter_map(|e| {
                let id = s(e, "id")?;
                let accent = e
                    .get("accents")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                    .and_then(|a| s(a, "locale"));
                Some(VoiceInfo {
                    name: s(e, "name").unwrap_or_else(|| id.clone()),
                    language: accent.or_else(|| s(e, "language")).unwrap_or_default(),
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
        let cursor = v
            .get("next_page")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .or_else(|| {
                v.get("data")?
                    .as_array()?
                    .last()?
                    .get("id")?
                    .as_str()
                    .map(str::to_string)
            })?;
        Some(self.with_key(HttpRequest::get(self.voices_url(Some(&cursor))), key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VOICE: &str = "v1";

    fn adapter(options: Value) -> Cartesia {
        Cartesia::new(
            &Profile::from_json(&json!({"id": "ca", "kind": "cartesia",
                "voice": VOICE, "model": "m1", "options": options}))
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

    fn body(code: &str) -> String {
        format!(
            r#"{{"error_code": "{code}", "title": "T", "message": "m {code}", "request_id": "r"}}"#
        )
    }

    #[test]
    fn error_mapping_follows_the_table() {
        let a = adapter(json!({}));
        let err = |s, b: &str| a.map_error(&reply(s, b), VOICE, None).reason;
        for (status, code, want) in [
            (401, "unauthorized", Reason::Auth),
            (402, "quota_exceeded", Reason::Quota),
            (403, "plan_upgrade_required", Reason::BadConfig),
            (403, "forbidden", Reason::Auth),
            (404, "voice_not_found", Reason::BadVoice),
            (422, "voice_model_mismatch", Reason::BadVoice),
            (404, "model_not_found", Reason::BadConfig),
            (422, "language_not_supported", Reason::BadConfig),
            (400, "invalid_request", Reason::BadConfig),
            (429, "concurrency_limited", Reason::RateLimited),
            (500, "internal", Reason::Server),
            (503, "unavailable", Reason::Server),
        ] {
            assert_eq!(err(status, &body(code)), want, "{status} {code}");
        }
        // Versions before 2026-03-01: plain "Title: Message" text.
        assert_eq!(err(401, "Unauthorized: Invalid API key"), Reason::Auth);
        assert_eq!(err(404, "Not Found: Voice not found"), Reason::BadVoice);
        assert_eq!(
            err(400, "Bad Request: Invalid Cartesia-Version"),
            Reason::BadConfig
        );
    }

    #[test]
    fn an_old_api_version_is_named_in_the_message() {
        let a = adapter(json!({"api_version": "2024-06-10"}));
        let e = a.map_error(
            &reply(400, "Bad Request: Unsupported Cartesia-Version header"),
            VOICE,
            None,
        );
        assert_eq!(e.status, Some(400));
        assert_eq!(
            e.message,
            "Cartesia settings do not work (400): Bad Request: Unsupported Cartesia-Version \
             header; check options.api_version (now 2024-06-10) and the model (m1)"
        );
        // An unknown or retired model is named (#235).
        let e = a.map_error(&reply(404, &body("model_not_found")), VOICE, None);
        assert!(
            e.message
                .starts_with("Cartesia does not know the model 'm1' (404)"),
            "{}",
            e.message
        );
        let e = a.map_error(&reply(401, &body("unauthorized")), VOICE, None);
        assert_eq!(e.message, "Cartesia refused the key (401): m unauthorized");
    }

    #[test]
    fn request_golden() {
        let a = adapter(json!({"api_version": "2025-04-16", "language": "de",
            "sample_rate": 44100}));
        let key = Secret::new("sk_car_test0123456789");
        let r = a.synth_request("Hallo.", VOICE, 250, Some(&key));
        assert_eq!(r.url, "https://api.cartesia.ai/tts/bytes");
        assert_eq!(
            r.header_value("authorization"),
            Some("Bearer sk_car_test0123456789")
        );
        assert_eq!(r.header_value("cartesia-version"), Some("2025-04-16"));
        let body: Value = serde_json::from_slice(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"model_id": "m1", "transcript": "Hallo.",
                "voice": {"id": VOICE},
                "output_format": {"container": "raw", "encoding": "pcm_s16le",
                    "sample_rate": 44100},
                "language": "de", "generation_config": {"speed": 1.25}})
        );
        assert_eq!(a.requested_rate(), Some(44_100));
        assert!(a.raw_pcm());
        let d = adapter(json!({}));
        let r = d.synth_request("Hi.", VOICE, 200, None);
        assert_eq!(r.header_value("authorization"), None);
        assert_eq!(r.header_value("cartesia-version"), Some("2026-08-14"));
        assert_eq!(
            d.body("Hi.", VOICE, 200),
            json!({"model_id": "m1", "transcript": "Hi.",
                "voice": {"id": VOICE},
                "output_format": {"container": "raw", "encoding": "pcm_s16le",
                    "sample_rate": 24000},
                "language": "en"})
        );
        assert_eq!(d.body("x", VOICE, 400)["generation_config"]["speed"], 1.5);
        assert_eq!(d.body("x", VOICE, 100)["generation_config"]["speed"], 0.6);
    }

    #[test]
    fn voice_pages() {
        let a = adapter(json!({}));
        let first = match a.voices(Some(&Secret::new("k"))) {
            VoiceSource::Fetch { request, .. } => request,
            VoiceSource::Fixed(_) => panic!("fetched"),
        };
        assert_eq!(first.url, "https://api.cartesia.ai/voices?limit=100");
        assert_eq!(first.header_value("authorization"), Some("Bearer k"));
        assert_eq!(first.header_value("cartesia-version"), Some("2026-08-14"));
        let page = br#"{"data": [
            {"id": "v1", "name": "Katie", "language": "en"},
            {"id": "v2", "name": "Hans", "language": "de",
             "accents": [{"locale": "de-DE"}]},
            {"name": "no id"}],
            "has_more": true, "next_page": "v2"}"#;
        assert_eq!(
            a.parse_voices(page).unwrap(),
            vec![
                VoiceInfo {
                    id: "v1".into(),
                    name: "Katie".into(),
                    language: "en".into()
                },
                VoiceInfo {
                    id: "v2".into(),
                    name: "Hans".into(),
                    language: "de-DE".into()
                },
            ]
        );
        assert_eq!(
            a.next_voices_page(page, None).unwrap().url,
            "https://api.cartesia.ai/voices?limit=100&starting_after=v2"
        );
        // No cursor named: the last id is the cursor.
        assert_eq!(
            a.next_voices_page(br#"{"data": [{"id": "z9"}], "has_more": true}"#, None)
                .unwrap()
                .url,
            "https://api.cartesia.ai/voices?limit=100&starting_after=z9"
        );
        assert!(a
            .next_voices_page(br#"{"data": [{"id": "z"}], "has_more": false}"#, None)
            .is_none());
        assert_eq!(
            a.parse_voices(br#"[{"id": "old", "name": "Old"}]"#)
                .unwrap()[0]
                .id,
            "old"
        );
    }
}
