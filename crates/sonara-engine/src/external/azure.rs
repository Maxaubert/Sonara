//! Kind `azure` (spec 5.4, 13.1, 13.2): Azure AI Speech, `POST
//! {base}/cognitiveservices/v1` with an SSML body, raw 16-bit mono PCM back
//! (`X-Microsoft-OutputFormat`). `{base}` is the profile's url, else
//! `https://{region}.tts.speech.microsoft.com`. The key goes in
//! `Ocp-Apim-Subscription-Key`. Error bodies are not documented as JSON, so
//! the status decides; a 400 for a voice that is not in the fetched list is
//! `bad_voice`.
use super::adapter::{
    key_allowed, Adapter, ErrorBody, HttpReply, HttpRequest, VoiceInfo, VoiceSource,
};
use super::error::{clean, headline, ExtError};
use super::keys::Secret;
use super::profile::{voice_locale, Kind, Profile, Url, AZURE_FORMATS};
use super::rate;
use super::split::Limit;
use crate::Reason;
use serde_json::Value;

/// The default output format.
pub const DEFAULT_FORMAT: &str = "raw-24khz-16bit-mono-pcm";
/// Azure asks every client to name itself.
pub const USER_AGENT: &str = concat!("Sonara/", env!("CARGO_PKG_VERSION"));

pub struct Azure {
    base: String,
    output_format: String,
    rate: u32,
    lang: Option<String>,
    label: String,
}

/// Text for SSML: `&`, `<`, `>`, `'` and `"` escaped (also right inside an
/// attribute quoted with either quote).
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            // XML 1.0 forbids most control characters; a reader chunk has
            // none, but a stray one must not break the request.
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

impl Azure {
    /// From a validated profile of kind `azure`.
    pub fn new(p: &Profile) -> Azure {
        let output_format = p
            .option_str("output_format")
            .unwrap_or(DEFAULT_FORMAT)
            .to_string();
        let rate = AZURE_FORMATS
            .iter()
            .find(|(n, _)| *n == output_format)
            .map(|(_, r)| *r)
            .unwrap_or(24_000);
        Azure {
            base: p.base_url().unwrap_or_default(),
            output_format,
            rate,
            lang: p.option_str("lang").map(str::to_string),
            label: p.display_label(),
        }
    }

    fn with_key(&self, mut req: HttpRequest, key: Option<&Secret>) -> HttpRequest {
        if let Some(k) = key.filter(|_| key_allowed(&req.url)) {
            req = req.header("Ocp-Apim-Subscription-Key", k.expose());
        }
        req.header("User-Agent", USER_AGENT)
    }

    /// The `xml:lang` of a request: the option, else the voice's locale,
    /// else `en-US`.
    fn lang_for(&self, voice: &str) -> String {
        self.lang
            .clone()
            .or_else(|| voice_locale(voice))
            .unwrap_or_else(|| "en-US".into())
    }

    /// The voice list: a resource host (`<resource>.cognitiveservices.
    /// azure.com`) serves it under `/tts`, a regional host at the root.
    fn voices_url(&self) -> String {
        let resource =
            Url::parse(&self.base).is_ok_and(|u| u.host.ends_with(".cognitiveservices.azure.com"));
        let prefix = if resource { "/tts" } else { "" };
        format!("{}{prefix}/cognitiveservices/voices/list", self.base)
    }

    /// The SSML body (tests check it).
    pub fn ssml(&self, text: &str, voice: &str, wpm: u32) -> String {
        let speed = rate::speed(Kind::Azure, wpm).unwrap_or(1.0);
        format!(
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{}'>\
             <voice name='{}'><prosody rate='{speed}'>{}</prosody></voice></speak>",
            xml_escape(&self.lang_for(voice)),
            xml_escape(voice),
            xml_escape(text)
        )
    }
}

impl Adapter for Azure {
    fn input_limit(&self) -> Limit {
        // Sonara's choice, well under Azure's 10-minute audio cap.
        Limit::Chars(2000)
    }

    fn synth_request(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
    ) -> HttpRequest {
        let req = HttpRequest {
            method: super::adapter::Method::Post,
            url: format!("{}/cognitiveservices/v1", self.base),
            headers: vec![
                ("Content-Type".into(), "application/ssml+xml".into()),
                (
                    "X-Microsoft-OutputFormat".into(),
                    self.output_format.clone(),
                ),
            ],
            body: Some(self.ssml(text, voice, wpm).into_bytes()),
        };
        self.with_key(req, key)
    }

    fn requested_rate(&self) -> Option<u32> {
        Some(self.rate)
    }

    fn raw_pcm(&self) -> bool {
        self.output_format.starts_with("raw-")
    }

    fn map_error(&self, reply: &HttpReply, voice: &str, listed: Option<bool>) -> ExtError {
        let s = reply.status;
        let reason = match s {
            401 | 403 => Reason::Auth,
            400 if !voice.is_empty() && listed == Some(false) => Reason::BadVoice,
            429 => Reason::RateLimited,
            500..=599 => Reason::Server,
            _ => Reason::BadConfig,
        };
        let detail = match ErrorBody::parse(&reply.body).message {
            Some(m) => clean(&m),
            None => match (s, reason) {
                (401, _) => "check the key and that it belongs to this region".into(),
                (_, Reason::BadVoice) => format!("'{voice}' is not in the voice list"),
                (400, _) => "check the voice name and the language".into(),
                _ => format!("HTTP {s}"),
            },
        };
        let mut e = ExtError::new(
            reason,
            format!("{} ({s}): {detail}", headline(reason, &self.label, "")),
        )
        .with_status(s);
        if matches!(s, 429 | 503) {
            e.retry_after = reply.retry_after;
        }
        e
    }

    fn voices(&self, key: Option<&Secret>) -> VoiceSource {
        VoiceSource::Fetch {
            request: self.with_key(HttpRequest::get(self.voices_url()), key),
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
        Ok(v.as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| {
                let id = s(e, "ShortName")?;
                let locale = s(e, "Locale").unwrap_or_default();
                let local = s(e, "LocalName")
                    .or_else(|| s(e, "DisplayName"))
                    .unwrap_or_else(|| id.clone());
                Some(VoiceInfo {
                    name: if locale.is_empty() {
                        local
                    } else {
                        format!("{local} ({locale})")
                    },
                    language: locale,
                    id,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn adapter(options: Value) -> Azure {
        let mut o = json!({"region": "westeurope"});
        for (k, v) in options.as_object().unwrap() {
            o[k] = v.clone();
        }
        Azure::new(
            &Profile::from_json(&json!({"id": "az", "kind": "azure",
                "voice": "en-US-VoiceANeural", "options": o}))
            .unwrap(),
        )
    }

    fn reply(status: u16, body: &str) -> HttpReply {
        HttpReply {
            status,
            content_type: None,
            retry_after: None,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn ssml_escapes_every_special_character() {
        assert_eq!(
            xml_escape(r#"Tom & Jerry <say> 'hi' "now""#),
            "Tom &amp; Jerry &lt;say&gt; &apos;hi&apos; &quot;now&quot;"
        );
        assert_eq!(
            xml_escape("Café, naïve, 日本語, 😀"),
            "Café, naïve, 日本語, 😀"
        );
        assert_eq!(xml_escape("a\u{1}b"), "a b");
        let a = adapter(json!({}));
        assert_eq!(
            a.ssml("1 < 2 & 'x'", "en-US-VoiceANeural", 250),
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
             <voice name='en-US-VoiceANeural'><prosody rate='1.25'>\
             1 &lt; 2 &amp; &apos;x&apos;</prosody></voice></speak>"
        );
        // A voice name cannot break out of its attribute.
        assert!(a
            .ssml("x", "a'/><b c='", 200)
            .contains("name='a&apos;/&gt;&lt;b c=&apos;'"));
    }

    #[test]
    fn lang_from_the_voice_or_the_option() {
        let a = adapter(json!({}));
        assert!(a
            .ssml("x", "de-DE-VoiceCNeural", 200)
            .contains("xml:lang='de-DE'"));
        assert!(a.ssml("x", "Custom", 200).contains("xml:lang='en-US'"));
        assert!(a.ssml("x", "x", 200).contains("rate='1'"));
        assert!(a.ssml("x", "x", 100).contains("rate='0.5'"));
        let fixed = adapter(json!({"lang": "en-GB"}));
        assert!(fixed
            .ssml("x", "de-DE-VoiceCNeural", 200)
            .contains("xml:lang='en-GB'"));
    }

    #[test]
    fn request_headers_and_url() {
        let a = adapter(json!({"output_format": "raw-16khz-16bit-mono-pcm"}));
        let r = a.synth_request(
            "Hi.",
            "en-US-VoiceBNeural",
            200,
            Some(&Secret::new("az-key")),
        );
        assert_eq!(
            r.url,
            "https://westeurope.tts.speech.microsoft.com/cognitiveservices/v1"
        );
        assert_eq!(r.header_value("ocp-apim-subscription-key"), Some("az-key"));
        assert_eq!(r.header_value("content-type"), Some("application/ssml+xml"));
        assert_eq!(
            r.header_value("x-microsoft-outputformat"),
            Some("raw-16khz-16bit-mono-pcm")
        );
        assert_eq!(r.header_value("user-agent"), Some(USER_AGENT));
        assert!(USER_AGENT.starts_with("Sonara/0."));
        assert_eq!(a.requested_rate(), Some(16_000));
        assert_eq!(adapter(json!({})).requested_rate(), Some(24_000));
        let by_url = Azure::new(
            &Profile::from_json(
                &json!({"id": "az", "kind": "azure", "voice": "en-US-VoiceBNeural",
                "url": "https://my-resource.cognitiveservices.azure.com"}),
            )
            .unwrap(),
        );
        assert_eq!(
            by_url.synth_request("a", "v", 200, None).url,
            "https://my-resource.cognitiveservices.azure.com/cognitiveservices/v1"
        );
        match by_url.voices(None) {
            VoiceSource::Fetch { request, .. } => assert_eq!(
                request.url,
                "https://my-resource.cognitiveservices.azure.com/tts/cognitiveservices/voices/list"
            ),
            VoiceSource::Fixed(_) => panic!("fetched"),
        }
        match adapter(json!({})).voices(None) {
            VoiceSource::Fetch { request, .. } => assert_eq!(
                request.url,
                "https://westeurope.tts.speech.microsoft.com/cognitiveservices/voices/list"
            ),
            VoiceSource::Fixed(_) => panic!("fetched"),
        }
    }

    #[test]
    fn error_mapping_follows_the_table() {
        let a = adapter(json!({}));
        let err = |s, listed| a.map_error(&reply(s, ""), "en-US-X", listed).reason;
        assert_eq!(err(401, None), Reason::Auth);
        assert_eq!(err(403, None), Reason::Auth);
        assert_eq!(err(400, None), Reason::BadConfig);
        assert_eq!(err(400, Some(true)), Reason::BadConfig);
        assert_eq!(err(400, Some(false)), Reason::BadVoice);
        assert_eq!(err(415, None), Reason::BadConfig);
        assert_eq!(err(429, None), Reason::RateLimited);
        for s in [500, 502, 503] {
            assert_eq!(err(s, None), Reason::Server);
        }
        let e = a.map_error(&reply(401, ""), "v", None);
        assert_eq!(
            e.message,
            "Azure Speech refused the key (401): check the key and that it belongs to this region"
        );
        let e = a.map_error(&reply(400, ""), "en-US-Nobody", Some(false));
        assert_eq!(
            e.message,
            "Azure Speech does not know this voice (400): 'en-US-Nobody' is not in the voice list"
        );
        let e = a.map_error(&reply(400, "SSML parse error at line 1"), "v", None);
        assert!(e.message.ends_with("(400): SSML parse error at line 1"));
    }

    #[test]
    fn voice_list_parsing() {
        let a = adapter(json!({}));
        let list = a
            .parse_voices(
                br#"[{"Name": "Microsoft Server Speech Text to Speech Voice (en-US, VoiceBNeural)",
                  "DisplayName": "Bea", "LocalName": "Bea", "ShortName": "en-US-VoiceBNeural",
                  "Gender": "Female", "Locale": "en-US"},
                 {"DisplayName": "Cea", "LocalName": "Cea", "ShortName": "de-DE-VoiceCNeural",
                  "Locale": "de-DE"},
                 {"LocalName": "no short name"}]"#,
            )
            .unwrap();
        assert_eq!(
            list,
            vec![
                VoiceInfo {
                    id: "en-US-VoiceBNeural".into(),
                    name: "Bea (en-US)".into(),
                    language: "en-US".into()
                },
                VoiceInfo {
                    id: "de-DE-VoiceCNeural".into(),
                    name: "Cea (de-DE)".into(),
                    language: "de-DE".into()
                },
            ]
        );
        assert_eq!(
            a.parse_voices(b"not json").unwrap_err().reason,
            Reason::Format
        );
    }
}
