//! Kind `gemini` (#235): the Gemini API's speech models, `POST {url}/v1beta/
//! models/{model}:streamGenerateContent?alt=sse` with `responseModalities:
//! ["AUDIO"]` (the generateContent family, not the Interactions API, which
//! stores each request for a day or more by default). Each server-sent
//! event is a `GenerateContentResponse` whose audio is base64
//! (`candidates[0].content.parts[].inlineData`), so the first audio plays
//! while the rest is made. A model that refuses the stream gets
//! `:generateContent` (the whole answer at once) from then on. Sonara asks
//! for headerless 16-bit PCM at 24 kHz (`responseFormat.audio.mimeType
//! AUDIO_L16`); WAV is read too. The key goes in the `x-goog-api-key`
//! header, never in the URL.
//!
//! - **No model or voice in code**: models and voices change upstream. The
//!   models come from `GET /v1beta/models` (those named `tts`), the voices
//!   from `GET /v1beta/voices` (the caller's stored voices and Google's
//!   prebuilt catalog), both live with the key. The model is required (it
//!   is in the URL); a profile without one says "choose a model".
//! - **Rate**: Gemini has no speed parameter; the rate is sent as a style
//!   (`speechMetadata.style`, `rate::gemini_pace`) after the profile's own
//!   `style`. Metadata, never part of the text (3.8 models read the text
//!   verbatim, so a spoken direction would be read aloud).
//! - **Older models**: a model that refuses `responseFormat` or
//!   `speechMetadata` (an unknown field is a 400 naming it) gets the same
//!   part once more without it, and never again (`adapt`, as Deepgram's
//!   `speed`); likewise the stream.
//! - **Requests**: the free tier counts requests, so Gemini takes whole
//!   messages by default (send mode `message`, #235): a reply is one
//!   request of up to `Profile::chunk_chars` (2000) characters; the input
//!   limit here is 4000 characters (the models take 8192 tokens).
//! - **429**: `quota` when Google names a daily or spend limit, else
//!   `rate_limited`; either way its `RetryInfo.retryDelay` (or
//!   `Retry-After`) is kept, so nothing is sent before it ends (`health`).
//!   A per-day limit waits for the daily reset (`daily_wait`).
use super::adapter::{encode, ModelInfo, ModelSource, StreamPiece, VoiceSource};
use super::adapter::{key_allowed, Adapter, ErrorBody, HttpReply, HttpRequest, VoiceInfo};
use super::audio::decode_body;
use super::error::{clean, headline, model_message, ExtError};
use super::keys::Secret;
use super::profile::Profile;
use super::rate;
use super::split::Limit;
use crate::{PcmChunk, Reason};
use base64::Engine as _;
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Gemini's default `chunk_chars` in send mode `message` (#235): a request
/// of up to 2000 characters is about two minutes of speech, well inside
/// the models' output limit.
pub const GEMINI_CHUNK_CHARS: u64 = 2000;

/// The rate Sonara asks Gemini for (24 kHz is its native rate).
pub const GEMINI_RATE: u32 = 24_000;
/// Voices and models asked for per page (the API's maximum is 1000).
const PAGE_SIZE: u32 = 1000;

/// Fields a model may refuse, as `ExtError::refused_param` names them.
const FORMAT_FIELD: &str = "responseFormat";
const STYLE_FIELD: &str = "speechMetadata";
/// The stream itself (`streamGenerateContent`).
const STREAM: &str = "streamGenerateContent";

pub struct Gemini {
    base: String,
    /// `None`: the profile names none yet (`Profile::missing_model`).
    model: Option<String>,
    language_code: Option<String>,
    style: Option<String>,
    label: String,
    /// The model refused `responseFormat`: it is not sent again.
    no_format: AtomicBool,
    /// The model refused `speechMetadata`: no style is sent again.
    no_style: AtomicBool,
    /// The model refused the stream: whole answers from then on.
    no_stream: AtomicBool,
}

impl Gemini {
    /// From a validated profile of kind `gemini`.
    pub fn new(p: &Profile) -> Gemini {
        Gemini {
            base: p.base_url().unwrap_or_default(),
            model: p.model.clone(),
            language_code: p.option_str("language_code").map(str::to_string),
            style: p
                .option_str("style")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            label: p.display_label(),
            no_format: AtomicBool::new(false),
            no_style: AtomicBool::new(false),
            no_stream: AtomicBool::new(false),
        }
    }

    fn with_key(&self, req: HttpRequest, key: Option<&Secret>) -> HttpRequest {
        match key.filter(|_| key_allowed(&req.url)) {
            Some(k) => req.header("x-goog-api-key", k.expose()),
            None => req,
        }
    }

    fn model(&self) -> &str {
        self.model.as_deref().unwrap_or_default()
    }

    /// The whole-answer URL (tests check it): never a key in it.
    pub fn url(&self) -> String {
        format!(
            "{}/v1beta/models/{}:generateContent",
            self.base,
            encode(self.model())
        )
    }

    /// The streaming URL: server-sent events (`alt=sse`).
    pub fn stream_url(&self) -> String {
        format!(
            "{}/v1beta/models/{}:{STREAM}?alt=sse",
            self.base,
            encode(self.model())
        )
    }

    /// The style sent for `wpm`: the profile's, then the pace.
    pub fn style_for(&self, wpm: u32) -> Option<String> {
        if self.no_style.load(Ordering::SeqCst) {
            return None;
        }
        let parts: Vec<&str> = self
            .style
            .as_deref()
            .into_iter()
            .chain(rate::gemini_pace(wpm))
            .collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    /// The request body (tests check it).
    pub fn body(&self, text: &str, voice: &str, wpm: u32) -> Value {
        let mut part = Map::new();
        part.insert("text".into(), json!(text));
        if let Some(style) = self.style_for(wpm) {
            part.insert(STYLE_FIELD.into(), json!({"style": style}));
        }
        // A stored or replicated voice is an id; a prebuilt one a name,
        // in the form every TTS model takes.
        let voice_config = if voice.starts_with("voice_") || voice.starts_with("voicekey_") {
            json!({"voice": voice})
        } else {
            json!({"prebuiltVoiceConfig": {"voiceName": voice}})
        };
        let mut speech = Map::new();
        speech.insert("voiceConfig".into(), voice_config);
        if let Some(l) = &self.language_code {
            speech.insert("languageCode".into(), json!(l));
        }
        let mut generation = Map::new();
        generation.insert("responseModalities".into(), json!(["AUDIO"]));
        generation.insert("speechConfig".into(), Value::Object(speech));
        if !self.no_format.load(Ordering::SeqCst) {
            generation.insert(
                FORMAT_FIELD.into(),
                json!({"audio": {"mimeType": "AUDIO_L16", "sampleRate": GEMINI_RATE}}),
            );
        }
        json!({"contents": [{"role": "user", "parts": [Value::Object(part)]}],
            "generationConfig": Value::Object(generation)})
    }

    /// Whether `responseFormat` is still sent.
    pub fn sends_format(&self) -> bool {
        !self.no_format.load(Ordering::SeqCst)
    }

    /// The audio of one `GenerateContentResponse` (a whole answer or one
    /// event of a stream): its `inlineData` parts, in order, as one chunk,
    /// or none; and why the model stopped, when it says. In a stream,
    /// `carry` holds the half sample an event ended with (Google may cut
    /// the PCM anywhere): it starts the next part, so no later sample is
    /// shifted by a byte.
    fn audio_of(
        &self,
        v: &Value,
        label: &str,
        mut carry: Option<&mut Vec<u8>>,
    ) -> Result<StreamPiece, ExtError> {
        let format = |why: &str| ExtError::new(Reason::Format, format!("{label} {why}"));
        let parts = v
            .pointer("/candidates/0/content/parts")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default();
        let mut out: Option<PcmChunk> = None;
        for data in parts.iter().filter_map(|p| p.get("inlineData")) {
            let b64 = data.get("data").and_then(Value::as_str).unwrap_or_default();
            let mut bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|_| format("sent audio that is not base64"))?;
            if let Some(c) = carry.as_deref_mut() {
                if !bytes.starts_with(b"RIFF") {
                    if !c.is_empty() {
                        bytes.splice(0..0, c.drain(..));
                    }
                    if bytes.len() % 2 == 1 {
                        c.extend(bytes.pop());
                    }
                }
                if bytes.is_empty() {
                    continue;
                }
            }
            let mime = data.get("mimeType").and_then(Value::as_str);
            let pcm = decode_body(&bytes, mime, Some(GEMINI_RATE), true, label)?;
            match &mut out {
                None => out = Some(pcm),
                Some(o) if o.sample_rate == pcm.sample_rate => o.samples.extend(pcm.samples),
                Some(_) => return Err(format("sent audio parts at different rates")),
            }
        }
        let note = v
            .pointer("/promptFeedback/blockReason")
            .or_else(|| v.pointer("/candidates/0/finishReason"))
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(StreamPiece { audio: out, note })
    }

    /// A page of `GET /v1beta/voices`, or of `/models` (`what`).
    fn list_url(&self, what: &str, token: Option<&str>) -> String {
        // The models list takes camelCase, the voices list snake_case.
        let (size, page) = match what {
            "models" => ("pageSize", "pageToken"),
            _ => ("page_size", "page_token"),
        };
        let mut url = format!("{}/v1beta/{what}?{size}={PAGE_SIZE}", self.base);
        if let Some(t) = token {
            url.push_str(&format!("&{page}={}", encode(t)));
        }
        url
    }

    fn json(&self, body: &[u8], what: &str) -> Result<Value, ExtError> {
        serde_json::from_slice(body).map_err(|e| {
            ExtError::new(
                Reason::Format,
                format!(
                    "the {what} list of {} is not JSON: {}",
                    self.label,
                    clean(&e.to_string())
                ),
            )
        })
    }
}

/// The next page token of a list (`next_page_token` or `nextPageToken`).
fn next_token(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    v.get("next_page_token")
        .or_else(|| v.get("nextPageToken"))
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// A field in snake_case or camelCase (`display_name`, `displayName`).
fn field<'a>(v: &'a Value, snake: &str, camel: &str) -> Option<&'a str> {
    v.get(snake)
        .or_else(|| v.get(camel))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// `RetryInfo.retryDelay` of a google.rpc.Status body (`"39s"`, `"1.5s"`).
pub fn retry_delay(body: &Value) -> Option<Duration> {
    body.pointer("/error/details")?
        .as_array()?
        .iter()
        .filter(|d| {
            d.get("@type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.ends_with("google.rpc.RetryInfo"))
        })
        .find_map(|d| {
            let s = d.get("retryDelay")?.as_str()?.trim().strip_suffix('s')?;
            s.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite() && *x >= 0.0)
                .map(Duration::from_secs_f64)
        })
}

/// How long a per-day 429 waits at `unix` (seconds): until the next
/// 08:00 UTC, which is midnight Pacific standard time, when Google resets
/// requests per day (an hour after the reset in summer time, never
/// before it); at least an hour, or Google's own wait when that is longer.
pub fn daily_wait(given: Option<Duration>, unix: u64) -> Duration {
    const DAY: u64 = 86_400;
    const RESET: u64 = 8 * 3600;
    let into = unix % DAY;
    let mut wait = (RESET + DAY - into) % DAY;
    if wait == 0 {
        wait = DAY;
    }
    let reset = Duration::from_secs(wait.max(3600));
    given.map_or(reset, |g| g.max(reset))
}

/// Which field a 400 refuses, from Google's unknown-field message
/// (`Unknown name "responseFormat" at 'generation_config'`).
fn refused_field(message: &str) -> Option<&'static str> {
    let m = message.to_ascii_lowercase();
    if !(m.contains("unknown name")
        || m.contains("cannot find field")
        || m.contains("not supported"))
    {
        return None;
    }
    if m.contains("responseformat") || m.contains("response_format") {
        Some(FORMAT_FIELD)
    } else if m.contains("speechmetadata") || m.contains("speech_metadata") {
        Some(STYLE_FIELD)
    } else {
        None
    }
}

/// Whether a 400's message says the voice does not exist.
fn unknown_voice(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("voice")
        && [
            "not found",
            "does not exist",
            "invalid voice",
            "unknown voice",
            "voice name",
        ]
        .iter()
        .any(|p| m.contains(p))
}

/// Whether a refusal is about the model (an unknown or retired one, or one
/// that makes no audio).
fn about_model(status: u16, message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    status == 404
        || (m.contains("model")
            && [
                "not found",
                "not supported",
                "does not support",
                "deprecated",
                "retired",
                "unknown",
            ]
            .iter()
            .any(|p| m.contains(p)))
}

/// Whether a 429 is a daily or spend limit (`quota`), not a per-minute one.
/// The quota id decides when Google sends one (its per-minute message also
/// says "check your plan and billing details", so billing alone is not a
/// sign).
fn daily_or_spend(raw: &str, message: &str) -> bool {
    if raw.contains("PerDay") {
        return true;
    }
    if raw.contains("PerMinute") || raw.contains("PerSecond") {
        return false;
    }
    let m = message.to_ascii_lowercase();
    ["per day", "daily", "spend", "credit"]
        .iter()
        .any(|p| m.contains(p))
        || raw.contains("\"quota_exceeded\"")
        || raw.contains("\"payment_required\"")
}

impl Adapter for Gemini {
    fn input_limit(&self) -> Limit {
        Limit::Chars(4000)
    }

    fn synth_request(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
    ) -> HttpRequest {
        self.with_key(
            HttpRequest::post_json(self.url(), &self.body(text, voice, wpm)),
            key,
        )
    }

    fn requested_rate(&self) -> Option<u32> {
        Some(GEMINI_RATE)
    }

    fn raw_pcm(&self) -> bool {
        true
    }

    /// The `inlineData` parts of the first candidate, in order, as one
    /// chunk. No audio at all (a blocked or empty generation) is `server`:
    /// it belongs to this text, so it must not block the engine.
    fn audio(&self, reply: &HttpReply, label: &str) -> Result<PcmChunk, ExtError> {
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| {
            ExtError::new(
                Reason::Format,
                format!("{label} sent an answer that is not JSON"),
            )
        })?;
        let piece = self.audio_of(&v, label, None)?;
        piece.audio.ok_or_else(|| no_audio(label, piece.note))
    }

    fn map_error(&self, reply: &HttpReply, _voice: &str, _listed: Option<bool>) -> ExtError {
        let s = reply.status;
        let eb = ErrorBody::parse(&reply.body);
        let raw = String::from_utf8_lossy(&reply.body);
        let message = eb.message.clone().unwrap_or_default();
        let lower = message.to_ascii_lowercase();
        let refused = (s == 400).then(|| refused_field(&message)).flatten();
        let key_invalid = raw.contains("API_KEY_INVALID") || lower.contains("api key not valid");
        let reason = match s {
            400 | 401 | 403 if key_invalid => Reason::Auth,
            401 | 403 => Reason::Auth,
            402 => Reason::Quota,
            400 if refused.is_none() && unknown_voice(&message) => Reason::BadVoice,
            429 if daily_or_spend(&raw, &message) => Reason::Quota,
            429 => Reason::RateLimited,
            500..=599 => Reason::Server,
            _ => Reason::BadConfig,
        };
        let text = if reason == Reason::BadConfig
            && refused.is_none()
            && self.model.is_some()
            && about_model(s, &message)
        {
            model_message(&self.label, self.model(), s, &eb.text_or(s))
        } else {
            format!(
                "{} ({s}): {}",
                headline(reason, &self.label, ""),
                eb.text_or(s)
            )
        };
        let mut e = ExtError::new(reason, text).with_status(s);
        if matches!(s, 429 | 503) {
            let body: Value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
            e.retry_after = reply.retry_after.or_else(|| retry_delay(&body));
            // A per-day limit ends at Google's daily reset, whatever short
            // retryDelay it carries: no probe every ten minutes all day.
            if s == 429 && raw.contains("PerDay") {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                e.retry_after = Some(daily_wait(e.retry_after, now));
            }
        }
        // A 400 or 404 of the stream that names it: the model may not
        // stream (or not exist; the whole-answer request then says so).
        if refused.is_none()
            && matches!(s, 400 | 404)
            && lower.contains(&STREAM.to_ascii_lowercase())
        {
            e.refused_param = Some(STREAM);
        } else {
            e.refused_param = refused;
        }
        e
    }

    /// A field the request carried and the model's error refused: send the
    /// part once more without it, and never again with it. The stream
    /// refused: the whole answer from then on.
    fn adapt(&self, request: &HttpRequest, error: &ExtError) -> bool {
        if error.reason != Reason::BadConfig {
            return false;
        }
        let body = request.body.as_deref().unwrap_or_default();
        let carried = |field: &str| String::from_utf8_lossy(body).contains(&format!("\"{field}\""));
        match error.refused_param {
            Some(FORMAT_FIELD) if carried(FORMAT_FIELD) => {
                !self.no_format.swap(true, Ordering::SeqCst)
            }
            Some(STYLE_FIELD) if carried(STYLE_FIELD) => {
                !self.no_style.swap(true, Ordering::SeqCst)
            }
            Some(STREAM) if request.url.contains(STREAM) => {
                !self.no_stream.swap(true, Ordering::SeqCst)
            }
            _ => false,
        }
    }

    /// `GET /v1beta/voices`: the caller's stored voices, then Google's
    /// prebuilt catalog, paged (`next_page_token`).
    fn voices(&self, key: Option<&Secret>) -> VoiceSource {
        VoiceSource::Fetch {
            request: self.with_key(HttpRequest::get(self.list_url("voices", None)), key),
            empty_on_error: false,
        }
    }

    fn parse_voices(&self, body: &[u8]) -> Result<Vec<VoiceInfo>, ExtError> {
        let v = self.json(body, "voice")?;
        Ok(v.get("voices")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|e| {
                // A stored voice's id (`voice_...`), or a prebuilt one's
                // speaker name.
                let id = field(e, "id", "id")?.to_string();
                let name = field(e, "display_name", "displayName").unwrap_or(&id);
                let name = match field(e, "description", "description") {
                    Some(d) if d.chars().count() <= 60 => format!("{name}, {d}"),
                    _ => name.to_string(),
                };
                let language = field(e, "language_code", "languageCode")
                    .unwrap_or_default()
                    .to_string();
                Some(VoiceInfo { id, name, language })
            })
            .collect())
    }

    fn next_voices_page(&self, body: &[u8], key: Option<&Secret>) -> Option<HttpRequest> {
        let token = next_token(body)?;
        Some(self.with_key(HttpRequest::get(self.list_url("voices", Some(&token))), key))
    }

    /// `GET /v1beta/models`, paged (`nextPageToken`): the speech models.
    fn models(&self, key: Option<&Secret>) -> Option<ModelSource> {
        Some(ModelSource {
            request: self.with_key(HttpRequest::get(self.list_url("models", None)), key),
            empty_on_error: false,
        })
    }

    /// The models whose id names `tts` (Google's speech models) and that
    /// take `generateContent` when the list says which methods they take.
    fn parse_models(&self, body: &[u8]) -> Result<Vec<ModelInfo>, ExtError> {
        let v = self.json(body, "model")?;
        Ok(v.get("models")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|m| {
                let name = field(m, "name", "name")?;
                let id = name.strip_prefix("models/").unwrap_or(name).to_string();
                if !id.to_ascii_lowercase().contains("tts") {
                    return None;
                }
                let methods = m
                    .get("supportedGenerationMethods")
                    .or_else(|| m.get("supported_generation_methods"))
                    .and_then(Value::as_array);
                if methods.is_some_and(|a| {
                    !a.iter()
                        .any(|x| x.as_str().is_some_and(|s| s.contains("enerateContent")))
                }) {
                    return None;
                }
                let display = field(m, "displayName", "display_name").unwrap_or(&id);
                Some(ModelInfo {
                    name: display.to_string(),
                    id,
                })
            })
            .collect())
    }

    fn next_models_page(&self, body: &[u8], key: Option<&Secret>) -> Option<HttpRequest> {
        let token = next_token(body)?;
        Some(self.with_key(HttpRequest::get(self.list_url("models", Some(&token))), key))
    }

    fn streams(&self) -> bool {
        !self.no_stream.load(Ordering::SeqCst)
    }

    fn stream_request(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
    ) -> Option<HttpRequest> {
        self.streams().then(|| {
            self.with_key(
                HttpRequest::post_json(self.stream_url(), &self.body(text, voice, wpm)),
                key,
            )
        })
    }

    /// One event: a `GenerateContentResponse` with the next audio, or an
    /// error object (a failure after the stream began).
    fn stream_piece(
        &self,
        carry: &mut Vec<u8>,
        data: &str,
        label: &str,
    ) -> Result<StreamPiece, ExtError> {
        let v: Value = serde_json::from_str(data).map_err(|_| {
            ExtError::new(
                Reason::Format,
                format!("{label} sent a stream event that is not JSON"),
            )
        })?;
        if let Some(err) = v.get("error").filter(|e| e.is_object()) {
            let status = err
                .get("code")
                .and_then(Value::as_u64)
                .and_then(|c| u16::try_from(c).ok())
                .filter(|c| (400..600).contains(c))
                .unwrap_or(500);
            return Err(self.map_error(
                &HttpReply {
                    status,
                    content_type: Some("application/json".into()),
                    retry_after: None,
                    body: data.as_bytes().to_vec(),
                },
                "",
                None,
            ));
        }
        self.audio_of(&v, label, Some(carry))
    }
}

/// No audio at all for a text: `server`, of this text only.
pub fn no_audio(label: &str, note: Option<String>) -> ExtError {
    ExtError::new(
        Reason::Server,
        format!(
            "{label} sent no audio for this text (reason: {})",
            note.as_deref().unwrap_or("none given")
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile with a model named (the user picks it; none in code).
    fn adapter(extra: Value) -> Gemini {
        let mut v = json!({"id": "ge", "kind": "gemini", "model": "tts-a"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        Gemini::new(&Profile::from_json(&v).unwrap())
    }

    fn reply(status: u16, body: &str) -> HttpReply {
        HttpReply {
            status,
            content_type: Some("application/json".into()),
            retry_after: None,
            body: body.as_bytes().to_vec(),
        }
    }

    fn rpc_error(code: u16, status: &str, message: &str, details: Value) -> String {
        json!({"error": {"code": code, "message": message, "status": status,
            "details": details}})
        .to_string()
    }

    fn audio_reply(parts: Value) -> String {
        json!({"candidates": [{"content": {"role": "model", "parts": parts},
            "finishReason": "STOP"}]})
        .to_string()
    }

    fn b64(samples: &[i16]) -> String {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn request_golden() {
        let a = adapter(json!({}));
        let r = a.synth_request("Hello.", "Voice1", 200, Some(&Secret::new("AIza-k")));
        assert_eq!(
            r.url,
            "https://generativelanguage.googleapis.com/v1beta/models/tts-a:generateContent"
        );
        // The stream: the same body, server-sent events, no key in the URL.
        let s = a
            .stream_request("Hello.", "Voice1", 200, Some(&Secret::new("AIza-k")))
            .unwrap();
        assert_eq!(
            s.url,
            "https://generativelanguage.googleapis.com/v1beta/models/tts-a:streamGenerateContent?alt=sse"
        );
        assert_eq!(s.body, r.body);
        assert_eq!(s.header_value("x-goog-api-key"), Some("AIza-k"));
        assert!(a.streams());
        assert!(!r.url.contains("key="), "the key never goes in the URL");
        assert_eq!(r.header_value("x-goog-api-key"), Some("AIza-k"));
        assert_eq!(r.header_value("authorization"), None);
        let body: Value = serde_json::from_slice(r.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"contents": [{"role": "user", "parts": [{"text": "Hello."}]}],
                "generationConfig": {"responseModalities": ["AUDIO"],
                    "speechConfig": {"voiceConfig": {"prebuiltVoiceConfig": {"voiceName": "Voice1"}}},
                    "responseFormat": {"audio": {"mimeType": "AUDIO_L16", "sampleRate": 24000}}}})
        );
        // The model, language, style and pace; a stored voice by id.
        let g = adapter(json!({"model": "tts-b",
            "options": {"language_code": "de-DE", "style": "calm and warm"}}));
        assert!(g.url().ends_with("/models/tts-b:generateContent"));
        let body = g.body("Hallo.", "voice_abc123", 320);
        assert_eq!(
            body["contents"][0]["parts"][0],
            json!({"text": "Hallo.", "speechMetadata": {"style": "calm and warm, speaking quickly"}})
        );
        assert_eq!(
            body["generationConfig"]["speechConfig"],
            json!({"voiceConfig": {"voice": "voice_abc123"}, "languageCode": "de-DE"})
        );
        assert_eq!(
            g.body("x", "voicekey_9", 200)["generationConfig"]["speechConfig"]["voiceConfig"],
            json!({"voice": "voicekey_9"})
        );
        assert_eq!(g.style_for(200).as_deref(), Some("calm and warm"));
        assert_eq!(a.style_for(100).as_deref(), Some("speaking slowly"));
        assert_eq!(a.style_for(200), None);
        // No key: no header.
        let r = a.synth_request("Hi.", "Voice2", 200, None);
        assert_eq!(r.header_value("x-goog-api-key"), None);
        assert_eq!(a.input_limit(), Limit::Chars(4000));
        assert_eq!(a.requested_rate(), Some(24_000));
        assert!(a.raw_pcm());
    }

    #[test]
    fn audio_is_base64_pcm_in_inline_data() {
        let a = adapter(json!({}));
        let ok = |parts: Value| a.audio(&reply(200, &audio_reply(parts)), "Gemini");
        let got = ok(
            json!([{"inlineData": {"mimeType": "audio/L16;codec=pcm;rate=24000",
            "data": b64(&[100, -200, 300])}}]),
        )
        .unwrap();
        assert_eq!(
            (got.samples, got.sample_rate),
            (vec![100, -200, 300], 24_000)
        );
        // A first sample of -1 (an MP3 frame sync) is still PCM.
        let got = ok(
            json!([{"inlineData": {"mimeType": "audio/L16;codec=pcm;rate=24000",
            "data": b64(&[-1, 0x0090, 5])}}]),
        )
        .unwrap();
        assert_eq!(got.samples, vec![-1, 0x0090, 5]);
        // The rate the reply names wins; several parts are one chunk.
        let got = ok(json!([
            {"inlineData": {"mimeType": "audio/L16;rate=16000", "data": b64(&[1, 2])}},
            {"text": "ignored"},
            {"inlineData": {"mimeType": "audio/L16;rate=16000", "data": b64(&[3])}}]))
        .unwrap();
        assert_eq!((got.samples, got.sample_rate), (vec![1, 2, 3], 16_000));
        // A 3.8 model's default WAV is read too.
        let wav = crate::wav::encode(&PcmChunk {
            samples: vec![7, 8],
            sample_rate: 24_000,
            channels: 1,
        });
        let got = ok(json!([{"inlineData": {"mimeType": "audio/wav",
            "data": base64::engine::general_purpose::STANDARD.encode(&wav)}}]))
        .unwrap();
        assert_eq!(got.samples, vec![7, 8]);
        // Compressed audio, bad base64 and not JSON are format errors.
        for parts in [
            json!([{"inlineData": {"mimeType": "audio/mp3", "data": b64(&[1, 2])}}]),
            json!([{"inlineData": {"mimeType": "audio/L16", "data": "@@@"}}]),
            json!([{"inlineData": {"mimeType": "audio/L16", "data": ""}}]),
        ] {
            assert_eq!(
                ok(parts.clone()).unwrap_err().reason,
                Reason::Format,
                "{parts}"
            );
        }
        assert_eq!(
            a.audio(&reply(200, "RIFF"), "Gemini").unwrap_err().reason,
            Reason::Format
        );
    }

    #[test]
    fn no_audio_is_a_server_failure_of_this_text_only() {
        let a = adapter(json!({}));
        for (body, why) in [
            (
                json!({"promptFeedback": {"blockReason": "PROHIBITED_CONTENT"}}),
                "PROHIBITED_CONTENT",
            ),
            (json!({"candidates": [{"finishReason": "OTHER"}]}), "OTHER"),
            (
                json!({"candidates": [{"content": {"parts": [{"text": "Sure!"}]},
                    "finishReason": "STOP"}]}),
                "STOP",
            ),
        ] {
            let e = a
                .audio(&reply(200, &body.to_string()), "Gemini")
                .unwrap_err();
            assert_eq!(e.reason, Reason::Server, "{body}");
            assert!(e.message.contains(why), "{}", e.message);
            assert!(e.reason.is_transient(), "never blocks the engine");
        }
    }

    #[test]
    fn error_mapping_follows_the_table() {
        let a = adapter(json!({}));
        let map = |s: u16, b: String| a.map_error(&reply(s, &b), "Voice1", None);
        let err = |s: u16, b: String| map(s, b).reason;
        let key_info = json!([{"@type": "type.googleapis.com/google.rpc.ErrorInfo",
            "reason": "API_KEY_INVALID", "domain": "googleapis.com"}]);
        assert_eq!(
            err(
                400,
                rpc_error(
                    400,
                    "INVALID_ARGUMENT",
                    "API key not valid. Please pass a valid API key.",
                    key_info
                )
            ),
            Reason::Auth
        );
        assert_eq!(
            err(
                403,
                rpc_error(
                    403,
                    "PERMISSION_DENIED",
                    "Method doesn't allow unregistered callers.",
                    json!([])
                )
            ),
            Reason::Auth
        );
        assert_eq!(
            err(401, rpc_error(401, "UNAUTHENTICATED", "x", json!([]))),
            Reason::Auth
        );
        // A voice the model does not know.
        assert_eq!(
            err(
                400,
                rpc_error(
                    400,
                    "INVALID_ARGUMENT",
                    "Voice name Nobody is not found.",
                    json!([])
                )
            ),
            Reason::BadVoice
        );
        assert_eq!(
            err(
                400,
                rpc_error(
                    400,
                    "INVALID_ARGUMENT",
                    "Invalid voice: Nobody does not exist",
                    json!([])
                )
            ),
            Reason::BadVoice
        );
        // Other 400s, a model that does not exist, a region or billing.
        assert_eq!(
            err(
                400,
                rpc_error(
                    400,
                    "INVALID_ARGUMENT",
                    "Request contains an invalid argument.",
                    json!([])
                )
            ),
            Reason::BadConfig
        );
        assert_eq!(
            err(
                400,
                rpc_error(
                    400,
                    "FAILED_PRECONDITION",
                    "User location is not supported for the API use.",
                    json!([])
                )
            ),
            Reason::BadConfig
        );
        // An unknown or retired model is named, with where to pick another.
        let e = map(
            404,
            rpc_error(
                404,
                "NOT_FOUND",
                "models/tts-a is not found for API version v1beta, or is not supported for generateContent.",
                json!([]),
            ),
        );
        assert_eq!(e.reason, Reason::BadConfig);
        assert!(
            e.message
                .starts_with("Gemini does not know the model 'tts-a' (404)"),
            "{}",
            e.message
        );
        assert!(e.message.contains("Choose another model"), "{}", e.message);
        assert_eq!(e.refused_param, None, "the whole answer: not the stream");
        let retired = map(
            400,
            rpc_error(
                400,
                "INVALID_ARGUMENT",
                "The model tts-a has been deprecated.",
                json!([]),
            ),
        );
        assert_eq!(retired.reason, Reason::BadConfig);
        assert!(retired.message.contains("'tts-a'"), "{}", retired.message);
        assert_eq!(
            err(
                402,
                r#"{"error": {"code": "payment_required", "message": "x"}}"#.into()
            ),
            Reason::Quota
        );
        assert_eq!(
            err(500, rpc_error(500, "INTERNAL", "x", json!([]))),
            Reason::Server
        );
        assert_eq!(
            err(
                503,
                rpc_error(503, "UNAVAILABLE", "The model is overloaded.", json!([]))
            ),
            Reason::Server
        );
        assert!(map(
            403,
            rpc_error(403, "PERMISSION_DENIED", "denied", json!([]))
        )
        .message
        .starts_with("Gemini refused the key (403): denied"));
    }

    #[test]
    fn a_429_keeps_its_retry_delay_and_tells_daily_from_per_minute() {
        let a = adapter(json!({}));
        let per_minute = rpc_error(
            429,
            "RESOURCE_EXHAUSTED",
            "You exceeded your current quota, please check your plan and billing details.",
            json!([
                {"@type": "type.googleapis.com/google.rpc.QuotaFailure",
                 "violations": [{"quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_requests",
                    "quotaId": "GenerateRequestsPerMinutePerProjectPerModel-FreeTier"}]},
                {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "39s"}]),
        );
        // "billing" in this message is Google's boilerplate: the quotaId
        // says per minute.
        let e = a.map_error(&reply(429, &per_minute), "Voice1", None);
        assert_eq!(e.status, Some(429));
        assert_eq!(e.retry_after, Some(Duration::from_secs(39)));
        let daily = rpc_error(
            429,
            "RESOURCE_EXHAUSTED",
            "You exceeded your current quota.",
            json!([
                {"@type": "type.googleapis.com/google.rpc.QuotaFailure",
                 "violations": [{"quotaId": "GenerateRequestsPerDayPerProjectPerModel-FreeTier"}]},
                {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "1.5s"}]),
        );
        let e = a.map_error(&reply(429, &daily), "Voice1", None);
        assert_eq!(e.reason, Reason::Quota);
        // A per-day limit waits for the daily reset, never its short
        // retryDelay (review of #235).
        assert!(
            e.retry_after
                .is_some_and(|d| d >= Duration::from_secs(3600)),
            "{:?}",
            e.retry_after
        );
        assert!(
            e.message.starts_with("Gemini is out of credit (429)"),
            "{}",
            e.message
        );
        // A Retry-After header wins over the body.
        let mut r = reply(429, &per_minute);
        r.retry_after = Some(Duration::from_secs(5));
        assert_eq!(
            a.map_error(&r, "Voice1", None).retry_after,
            Some(Duration::from_secs(5))
        );
        // No delay given: none kept.
        let bare = rpc_error(
            429,
            "RESOURCE_EXHAUSTED",
            "Resource has been exhausted.",
            json!([]),
        );
        let e = a.map_error(&reply(429, &bare), "Voice1", None);
        assert_eq!((e.reason, e.retry_after), (Reason::RateLimited, None));
        assert_eq!(
            retry_delay(&json!({"error": {"details": [
            {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "x"}]}})),
            None
        );
        assert!(super::daily_or_spend("", "Quota exceeded per day"));
        assert!(!super::daily_or_spend("", "Too many requests"));
    }

    #[test]
    fn a_daily_limit_waits_until_the_pacific_midnight_reset() {
        // RPD resets at midnight Pacific; 08:00 UTC is that midnight in
        // winter and an hour after it in summer, so never too early. At
        // least an hour, and a longer wait Google gives is kept.
        let h = |x: u64| Duration::from_secs(x * 3600);
        let day = 20_000 * 86_400; // a midnight UTC
        assert_eq!(daily_wait(None, day), h(8));
        assert_eq!(daily_wait(None, day + 7 * 3600), h(1));
        assert_eq!(
            daily_wait(None, day + 7 * 3600 + 1800),
            h(1),
            "an hour at least"
        );
        assert_eq!(daily_wait(None, day + 8 * 3600), h(24));
        assert_eq!(daily_wait(None, day + 9 * 3600), h(23));
        assert_eq!(daily_wait(Some(h(30)), day + 9 * 3600), h(30));
    }

    #[test]
    fn per_minute_billing_boilerplate_is_not_quota() {
        // Google's per-minute 429 says "check your plan and billing
        // details"; only the quota id or a daily/spend wording is quota.
        let a = adapter(json!({}));
        let body = rpc_error(
            429,
            "RESOURCE_EXHAUSTED",
            "You exceeded your current quota, please check your plan and billing details.",
            json!([{"@type": "type.googleapis.com/google.rpc.QuotaFailure",
                "violations": [{"quotaId": "GenerateRequestsPerMinutePerProjectPerModel-FreeTier"}]}]),
        );
        assert_eq!(
            a.map_error(&reply(429, &body), "Voice1", None).reason,
            Reason::RateLimited
        );
    }

    #[test]
    fn a_refused_field_is_dropped_once_and_remembered() {
        let a = adapter(json!({"options": {"style": "warm"}}));
        let refused = |field: &str| {
            rpc_error(400, "INVALID_ARGUMENT",
                &format!("Invalid JSON payload received. Unknown name \"{field}\" at 'generation_config': Cannot find field."),
                json!([]))
        };
        let req = a.synth_request("Hi.", "Voice1", 200, None);
        let e = a.map_error(&reply(400, &refused("responseFormat")), "Voice1", None);
        assert_eq!(
            (e.reason, e.refused_param),
            (Reason::BadConfig, Some("responseFormat"))
        );
        assert!(a.adapt(&req, &e), "dropped once");
        assert!(!a.sends_format());
        let again = a.synth_request("Hi.", "Voice1", 200, None);
        let body: Value = serde_json::from_slice(again.body.as_deref().unwrap()).unwrap();
        assert!(body["generationConfig"].get("responseFormat").is_none());
        assert!(!a.adapt(&again, &e), "never twice");
        // The style the same way.
        let e = a.map_error(&reply(400, &refused("speech_metadata")), "Voice1", None);
        assert_eq!(e.refused_param, Some("speechMetadata"));
        assert!(a.adapt(&again, &e));
        assert_eq!(a.style_for(400), None);
        let body = a.body("Hi.", "Voice1", 400);
        assert_eq!(body["contents"][0]["parts"][0], json!({"text": "Hi."}));
        // A 400 that names no field is never adapted.
        let b = adapter(json!({}));
        let other = b.map_error(
            &reply(
                400,
                &rpc_error(400, "INVALID_ARGUMENT", "Bad text.", json!([])),
            ),
            "Voice1",
            None,
        );
        assert_eq!(other.refused_param, None);
        assert!(!b.adapt(&b.synth_request("x", "Voice1", 200, None), &other));
    }

    #[test]
    fn voices_come_live_from_the_voices_list() {
        // No voice list in code (#235): `GET /v1beta/voices`, paged.
        let a = adapter(json!({}));
        let key = Secret::new("AIza-k");
        let r = match a.voices(Some(&key)) {
            VoiceSource::Fetch {
                request,
                empty_on_error,
            } => {
                assert!(!empty_on_error);
                request
            }
            VoiceSource::Fixed(_) => panic!("fetched"),
        };
        assert_eq!(
            r.url,
            "https://generativelanguage.googleapis.com/v1beta/voices?page_size=1000"
        );
        assert_eq!(r.header_value("x-goog-api-key"), Some("AIza-k"));
        // Both JSON spellings; a stored voice by id, a prebuilt one by name.
        let page = br#"{"voices": [
            {"id": "voice_abc", "display_name": "My voice", "language_code": "en-US", "type": "replicated"},
            {"id": "Voice1", "displayName": "Voice1", "languageCode": "de-DE",
             "description": "Firm"},
            {"display_name": "no id"}],
            "next_page_token": "t 2"}"#;
        let v = a.parse_voices(page).unwrap();
        assert_eq!(
            v,
            vec![
                VoiceInfo {
                    id: "voice_abc".into(),
                    name: "My voice".into(),
                    language: "en-US".into()
                },
                VoiceInfo {
                    id: "Voice1".into(),
                    name: "Voice1, Firm".into(),
                    language: "de-DE".into()
                },
            ]
        );
        let next = a.next_voices_page(page, Some(&key)).unwrap();
        assert_eq!(
            next.url,
            "https://generativelanguage.googleapis.com/v1beta/voices?page_size=1000&page_token=t%202"
        );
        assert_eq!(next.header_value("x-goog-api-key"), Some("AIza-k"));
        assert!(a.next_voices_page(br#"{"voices": []}"#, None).is_none());
        assert!(a
            .next_voices_page(br#"{"voices": [], "nextPageToken": ""}"#, None)
            .is_none());
        assert_eq!(
            a.parse_voices(b"<html>").unwrap_err().reason,
            Reason::Format
        );
    }

    #[test]
    fn models_come_live_and_only_the_speech_ones() {
        let a = adapter(json!({}));
        let src = a.models(Some(&Secret::new("AIza-k"))).unwrap();
        assert_eq!(
            src.request.url,
            "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000"
        );
        assert_eq!(src.request.header_value("x-goog-api-key"), Some("AIza-k"));
        let page = br#"{"models": [
            {"name": "models/text-model", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/new-tts", "displayName": "New TTS",
             "supportedGenerationMethods": ["generateContent", "countTokens"]},
            {"name": "models/embed-tts-like", "supportedGenerationMethods": ["embedContent"]},
            {"name": "models/old-TTS"}],
            "nextPageToken": "p2"}"#;
        assert_eq!(
            a.parse_models(page).unwrap(),
            vec![
                ModelInfo {
                    id: "new-tts".into(),
                    name: "New TTS".into()
                },
                ModelInfo::named("old-TTS"),
            ]
        );
        assert_eq!(
            a.next_models_page(page, None).unwrap().url,
            "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000&pageToken=p2"
        );
    }

    #[test]
    fn stream_events_carry_audio_a_finish_reason_or_an_error() {
        let a = adapter(json!({}));
        let event = |parts: Value| audio_reply(parts);
        let p = a
            .stream_piece(
                &mut Vec::new(),
                &event(
                    json!([{"inlineData": {"mimeType": "audio/L16;codec=pcm;rate=24000",
                    "data": b64(&[1, 2, 3])}}]),
                ),
                "Gemini",
            )
            .unwrap();
        assert_eq!(p.audio.unwrap().samples, vec![1, 2, 3]);
        assert_eq!(p.note.as_deref(), Some("STOP"));
        let empty = a
            .stream_piece(
                &mut Vec::new(),
                r#"{"candidates": [{"content": {"parts": []}}]}"#,
                "Gemini",
            )
            .unwrap();
        assert_eq!(empty, StreamPiece::default());
        let e = a
            .stream_piece(
                &mut Vec::new(),
                &rpc_error(429, "RESOURCE_EXHAUSTED", "slow down", json!([])),
                "Gemini",
            )
            .unwrap_err();
        assert_eq!((e.reason, e.status), (Reason::RateLimited, Some(429)));
        assert_eq!(
            a.stream_piece(&mut Vec::new(), "not json", "Gemini")
                .unwrap_err()
                .reason,
            Reason::Format
        );
    }

    #[test]
    fn a_sample_split_across_events_is_joined_not_shifted() {
        // Review of #235: each event was decoded alone and an odd last
        // byte dropped, so a sample split across two events shifted every
        // later sample by one byte (loud noise).
        let a = adapter(json!({}));
        let bytes: Vec<u8> = [1i16, -2, 3, 300]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let event = |b: &[u8]| {
            audio_reply(
                json!([{"inlineData": {"mimeType": "audio/L16;codec=pcm;rate=24000",
                "data": base64::engine::general_purpose::STANDARD.encode(b)}}]),
            )
        };
        let mut carry = Vec::new();
        let p = a
            .stream_piece(&mut carry, &event(&bytes[..3]), "G")
            .unwrap();
        assert_eq!(p.audio.unwrap().samples, vec![1]);
        assert_eq!(carry.len(), 1, "the half sample waits for the next event");
        // One byte alone: still half a sample, no audio yet.
        let p = a
            .stream_piece(&mut carry, &event(&bytes[3..4]), "G")
            .unwrap();
        assert_eq!(p.audio.unwrap().samples, vec![-2]);
        assert!(carry.is_empty());
        let p = a
            .stream_piece(&mut carry, &event(&bytes[4..5]), "G")
            .unwrap();
        assert_eq!(p.audio, None);
        assert_eq!(carry.len(), 1);
        let p = a
            .stream_piece(&mut carry, &event(&bytes[5..]), "G")
            .unwrap();
        assert_eq!(p.audio.unwrap().samples, vec![3, 300]);
        assert!(carry.is_empty());
    }

    #[test]
    fn a_refused_stream_falls_back_to_whole_answers_once() {
        let a = adapter(json!({}));
        let req = a.stream_request("Hi.", "Voice1", 200, None).unwrap();
        let e = a.map_error(
            &reply(
                404,
                &rpc_error(
                    404,
                    "NOT_FOUND",
                    "models/tts-a is not found for API version v1beta, or is not supported for streamGenerateContent.",
                    json!([]),
                ),
            ),
            "Voice1",
            None,
        );
        assert_eq!(e.refused_param, Some("streamGenerateContent"));
        assert!(a.adapt(&req, &e), "the stream is dropped once");
        assert!(!a.streams());
        assert!(a.stream_request("Hi.", "Voice1", 200, None).is_none());
        assert!(!a.adapt(&req, &e), "never twice");
        // The same refusal of a whole answer is not adapted.
        let whole = a.synth_request("Hi.", "Voice1", 200, None);
        assert!(!a.adapt(&whole, &e));
    }
}
