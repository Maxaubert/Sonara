//! How a provider is spoken to: an `Adapter` turns a chunk into one HTTP
//! request and maps the reply's errors; `execute` runs a request with the
//! profile's agent; `ErrorBody` reads the error shapes the researched
//! servers send (spec 13.2).
use super::audio::decode_body;
use super::error::{clean, ExtError};
use super::keys::Secret;
use super::profile::Url;
use super::split::Limit;
use crate::{PcmChunk, Reason};
use serde_json::Value;
use std::time::Duration;

/// Largest audio body read (64 MiB).
pub const MAX_AUDIO_BODY: u64 = 64 * 1024 * 1024;
/// Largest error or voice-list body read (16 KiB for errors, 4 MiB lists).
pub const MAX_ERROR_BODY: u64 = 16 * 1024;
pub const MAX_LIST_BODY: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// One request. `Debug` leaves the headers out (one may hold the key).
#[derive(Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl HttpRequest {
    pub fn get(url: String) -> HttpRequest {
        HttpRequest {
            method: Method::Get,
            url,
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn post_json(url: String, body: &Value) -> HttpRequest {
        HttpRequest {
            method: Method::Post,
            url,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Some(body.to_string().into_bytes()),
        }
    }

    pub fn header(mut self, k: &str, v: &str) -> HttpRequest {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }

    /// The value of a header (tests).
    pub fn header_value(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

/// A key may go to `url` only over https, or over http to a loopback host
/// (spec 6.3). The query and fragment do not change the host, so they are
/// left out of the check (a voice list URL may carry `?model=`).
pub fn key_allowed(url: &str) -> bool {
    let base = url.split(['?', '#']).next().unwrap_or_default();
    Url::parse(base).is_ok_and(|u| u.https || u.is_loopback())
}

/// A provider's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpReply {
    pub status: u16,
    pub content_type: Option<String>,
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

impl HttpReply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// One voice as a provider lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
}

impl VoiceInfo {
    pub fn named(id: &str) -> VoiceInfo {
        VoiceInfo {
            id: id.to_string(),
            name: id.to_string(),
            language: String::new(),
        }
    }
}

/// Where the voice list comes from.
pub enum VoiceSource {
    Fixed(Vec<VoiceInfo>),
    /// Fetch it; with `empty_on_error`, a failed fetch is an empty list.
    Fetch {
        request: HttpRequest,
        empty_on_error: bool,
    },
}

/// One provider shape.
pub trait Adapter: Send + Sync {
    fn input_limit(&self) -> Limit;
    /// The request for one part of a chunk. `key` is `None` when no key
    /// resolves or it may not be sent to this URL.
    fn synth_request(&self, text: &str, voice: &str, wpm: u32, key: Option<&Secret>)
        -> HttpRequest;
    /// The rate asked for, for raw PCM without a rate in its reply.
    fn requested_rate(&self) -> Option<u32>;
    /// The reason and message of a non-2xx reply. `listed` says whether
    /// `voice` is in the last fetched voice list (`None`: no list yet).
    fn map_error(&self, reply: &HttpReply, voice: &str, listed: Option<bool>) -> ExtError;
    /// The audio of a 2xx reply: by default the body is WAV or raw PCM
    /// (Google wraps it in JSON).
    fn audio(&self, reply: &HttpReply, label: &str) -> Result<PcmChunk, ExtError> {
        decode_body(
            &reply.body,
            reply.content_type.as_deref(),
            self.requested_rate(),
            label,
        )
    }
    fn voices(&self, key: Option<&Secret>) -> VoiceSource;
    fn parse_voices(&self, body: &[u8]) -> Result<Vec<VoiceInfo>, ExtError>;
    /// The request for the next page of a paged voice list (ElevenLabs),
    /// from the body of the page just read; `None` when it was the last.
    fn next_voices_page(&self, _body: &[u8], _key: Option<&Secret>) -> Option<HttpRequest> {
        None
    }
}

/// Percent-encode a query value or a path segment.
pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Run one request with `agent`; transport failures are `network` or
/// `timeout`, never a message with a header in it.
pub fn execute(
    agent: &ureq::Agent,
    req: &HttpRequest,
    max_body: u64,
    host: &str,
) -> Result<HttpReply, ExtError> {
    let result = match req.method {
        Method::Get => {
            let mut b = agent.get(&req.url);
            for (k, v) in &req.headers {
                b = b.header(k.as_str(), v.as_str());
            }
            b.call()
        }
        Method::Post => {
            let mut b = agent.post(&req.url);
            for (k, v) in &req.headers {
                b = b.header(k.as_str(), v.as_str());
            }
            b.send(req.body.as_deref().unwrap_or_default())
        }
    };
    let mut resp = result.map_err(|e| transport(e, host))?;
    let status = resp.status().as_u16();
    let header = |n: &str| {
        resp.headers()
            .get(n)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let content_type = header("content-type");
    let retry_after = header("retry-after").and_then(|v| {
        v.trim()
            .parse::<f64>()
            .ok()
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(Duration::from_secs_f64)
    });
    let limit = if (200..300).contains(&status) {
        max_body
    } else {
        MAX_ERROR_BODY
    };
    let body = resp
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| match e {
            ureq::Error::BodyExceedsLimit(_) => ExtError::new(
                Reason::Format,
                format!("the answer of {host} is larger than {limit} bytes"),
            ),
            other => transport(other, host),
        });
    let body = match body {
        Ok(b) => b,
        // An error body that cannot be read still has its status.
        Err(_) if !(200..300).contains(&status) => Vec::new(),
        Err(e) => return Err(e),
    };
    Ok(HttpReply {
        status,
        content_type,
        retry_after,
        body,
    })
}

fn transport(e: ureq::Error, host: &str) -> ExtError {
    match e {
        ureq::Error::Timeout(_) => {
            ExtError::new(Reason::Timeout, format!("{host} did not answer in time"))
        }
        other => ExtError::new(Reason::Network, format!("cannot reach {host}: {other}")),
    }
}

/// What an error body says, in the shapes of OpenAI, FastAPI servers
/// (`detail`), Chatterbox (`detail.error`) and LocalAI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorBody {
    pub message: Option<String>,
    pub code: Option<String>,
    pub kind: Option<String>,
    pub param: Option<String>,
    pub status: Option<String>,
}

fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl ErrorBody {
    pub fn parse(body: &[u8]) -> ErrorBody {
        let Ok(v) = serde_json::from_slice::<Value>(body) else {
            let t = String::from_utf8_lossy(body).trim().to_string();
            return ErrorBody {
                message: (!t.is_empty() && !t.starts_with('<')).then_some(t),
                ..Default::default()
            };
        };
        Self::from_value(&v)
    }

    fn from_object(o: &Value) -> ErrorBody {
        if let Some(inner) = o.get("error").filter(|e| e.is_object()) {
            return Self::from_object(inner);
        }
        ErrorBody {
            message: text(o.get("message"))
                .or_else(|| text(o.get("msg")))
                .or_else(|| text(o.get("error")))
                .or_else(|| text(o.get("title"))),
            code: text(o.get("code")).or_else(|| text(o.get("error_code"))),
            kind: text(o.get("type")),
            param: text(o.get("param")),
            status: text(o.get("status")),
        }
    }

    fn from_value(v: &Value) -> ErrorBody {
        if let Some(d) = v.get("detail") {
            return match d {
                Value::String(s) => ErrorBody {
                    message: Some(s.clone()),
                    ..Default::default()
                },
                Value::Array(items) => {
                    // FastAPI validation errors: [{loc, msg, type}].
                    let msgs: Vec<String> = items
                        .iter()
                        .filter_map(|i| {
                            let loc = i
                                .get("loc")
                                .and_then(Value::as_array)
                                .map(|l| {
                                    l.iter()
                                        .filter_map(|x| x.as_str())
                                        .collect::<Vec<_>>()
                                        .join(".")
                                })
                                .unwrap_or_default();
                            let msg = text(i.get("msg"))?;
                            Some(if loc.is_empty() {
                                msg
                            } else {
                                format!("{loc}: {msg}")
                            })
                        })
                        .collect();
                    let param = items.iter().find_map(|i| {
                        i.get("loc")?
                            .as_array()?
                            .last()?
                            .as_str()
                            .map(str::to_string)
                    });
                    ErrorBody {
                        message: (!msgs.is_empty()).then(|| msgs.join("; ")),
                        param,
                        ..Default::default()
                    }
                }
                Value::Object(_) => Self::from_object(d),
                _ => ErrorBody::default(),
            };
        }
        match v {
            Value::Object(_) => Self::from_object(v),
            _ => ErrorBody::default(),
        }
    }

    /// Whether the error is about the voice (the message or `param`).
    pub fn mentions(&self, word: &str) -> bool {
        let has = |s: &Option<String>| {
            s.as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(word))
        };
        has(&self.message) || has(&self.param) || has(&self.code)
    }

    /// The provider's message, cleaned, or the status line.
    pub fn text_or(&self, status: u16) -> String {
        match &self.message {
            Some(m) => clean(m),
            None => format!("HTTP {status}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_shapes_of_the_researched_servers() {
        let openai = ErrorBody::parse(
            br#"{"error": {"message": "Incorrect API key", "type": "invalid_request_error",
                "code": "invalid_api_key", "param": null}}"#,
        );
        assert_eq!(openai.message.as_deref(), Some("Incorrect API key"));
        assert_eq!(openai.code.as_deref(), Some("invalid_api_key"));
        let fastapi = ErrorBody::parse(br#"{"detail": "Voice 'xx' not found"}"#);
        assert!(fastapi.mentions("voice"));
        let detail_obj = ErrorBody::parse(
            br#"{"detail": {"error": "validation_error", "message": "Invalid voice"}}"#,
        );
        assert_eq!(detail_obj.message.as_deref(), Some("Invalid voice"));
        let chatterbox = ErrorBody::parse(
            br#"{"detail": {"error": {"message": "boom", "type": "server_error"}}}"#,
        );
        assert_eq!(chatterbox.message.as_deref(), Some("boom"));
        assert_eq!(chatterbox.kind.as_deref(), Some("server_error"));
        let validation = ErrorBody::parse(
            br#"{"detail": [{"loc": ["body", "voice"], "msg": "field required", "type": "missing"}]}"#,
        );
        assert_eq!(validation.param.as_deref(), Some("voice"));
        assert_eq!(
            validation.message.as_deref(),
            Some("body.voice: field required")
        );
        let localai = ErrorBody::parse(
            br#"{"error": {"code": 500, "message": "model loading", "type": ""}}"#,
        );
        assert_eq!(localai.code.as_deref(), Some("500"));
        let plain = ErrorBody::parse(b"Service Unavailable");
        assert_eq!(plain.message.as_deref(), Some("Service Unavailable"));
        assert_eq!(ErrorBody::parse(b"<html>x</html>").message, None);
        assert_eq!(ErrorBody::parse(b"").text_or(502), "HTTP 502");
    }

    #[test]
    fn keys_go_only_over_https_or_to_loopback() {
        assert!(key_allowed("https://api.openai.com/v1/audio/speech"));
        assert!(key_allowed("http://127.0.0.1:8880/v1/audio/speech"));
        assert!(key_allowed("http://localhost/v1"));
        assert!(!key_allowed("http://10.0.0.5:8880/v1/audio/speech"));
        assert!(!key_allowed("http://example.com/v1"));
        // A query or fragment does not change where the request goes.
        assert!(key_allowed(
            "http://127.0.0.1:8080/v1/audio/voices?model=a%20b"
        ));
        assert!(key_allowed("https://tts.example.com/v1/voices?page=2#x"));
        assert!(!key_allowed("http://10.0.0.5/v1/voices?model=m"));
        assert!(key_allowed(
            "https://api.elevenlabs.io/v1/text-to-speech/x?output_format=pcm_24000"
        ));
        assert!(!key_allowed("http://example.com/v1?x=https://"));
    }

    #[test]
    fn debug_never_shows_headers() {
        let r = HttpRequest::get("https://x/v1".into()).header("Authorization", "Bearer sk-1");
        assert!(!format!("{r:?}").contains("sk-1"));
    }
}
