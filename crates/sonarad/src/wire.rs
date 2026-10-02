//! JSON shapes of protocol v1 core: replies, errors and events. Field names
//! are exactly those of spec section 4.1.
use serde_json::{json, Map, Value};
use sonara_engine::LicenseClass;
use sonara_reader::{ItemPhase, State, Value as SettingValue, Voice};

/// Error codes (spec section 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Auth,
    BadRequest,
    UnknownType,
    Unsupported,
    Incompatible,
    Busy,
    Engine,
    NotFound,
}

impl Code {
    pub fn as_str(&self) -> &'static str {
        match self {
            Code::Auth => "E_AUTH",
            Code::BadRequest => "E_BAD_REQUEST",
            Code::UnknownType => "E_UNKNOWN_TYPE",
            Code::Unsupported => "E_UNSUPPORTED",
            Code::Incompatible => "E_INCOMPATIBLE",
            Code::Busy => "E_BUSY",
            Code::Engine => "E_ENGINE",
            Code::NotFound => "E_NOT_FOUND",
        }
    }

    /// The HTTP status of an error reply on the HTTP transport.
    pub fn http_status(&self) -> u16 {
        match self {
            Code::Auth => 401,
            Code::UnknownType | Code::NotFound => 404,
            Code::Busy | Code::Incompatible => 409,
            Code::Engine => 500,
            Code::BadRequest | Code::Unsupported => 400,
        }
    }
}

/// A failed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: Code,
    pub message: String,
}

impl Failure {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Failure {
            code,
            message: message.into(),
        }
    }
}

/// `{id?, ok: true, ...fields}`. `id` is echoed only when the request had one.
pub fn ok_reply(id: Option<&Value>, fields: Map<String, Value>) -> Value {
    let mut m = Map::new();
    if let Some(id) = id {
        m.insert("id".into(), id.clone());
    }
    m.insert("ok".into(), Value::Bool(true));
    m.extend(fields);
    Value::Object(m)
}

/// `{id?, ok: false, error: {code, message}}`.
pub fn error_reply(id: Option<&Value>, f: &Failure) -> Value {
    let mut m = Map::new();
    if let Some(id) = id {
        m.insert("id".into(), id.clone());
    }
    m.insert("ok".into(), Value::Bool(false));
    m.insert(
        "error".into(),
        json!({"code": f.code.as_str(), "message": f.message}),
    );
    Value::Object(m)
}

pub fn phase_str(p: ItemPhase) -> &'static str {
    match p {
        ItemPhase::Started => "started",
        ItemPhase::Finished => "finished",
        ItemPhase::Skipped => "skipped",
        ItemPhase::Failed => "failed",
    }
}

/// The `state` event. `engine_status` names the current engine; readiness
/// and download progress are added by a later minor (spec section 5).
pub fn state_event(s: &State, engine: &str) -> Value {
    let now_playing = match &s.now_playing {
        None => Value::Null,
        Some(n) => json!({
            "item_id": n.item_id.0,
            "label": n.label,
            "text": n.text,
            "chunk": n.chunk,
            "chunks": n.chunks,
        }),
    };
    json!({
        "event": "state",
        "seq": s.seq,
        "now_playing": now_playing,
        "queued": s.queued,
        "paused": s.paused,
        "muted": s.muted,
        "volume": s.volume,
        "rate": s.rate,
        "voice": s.voice,
        "engine_status": {"engine": engine},
    })
}

pub fn item_event(item_id: u64, phase: ItemPhase) -> Value {
    json!({"event": "item", "item_id": item_id, "phase": phase_str(phase)})
}

pub fn log_event(message: &str) -> Value {
    json!({"event": "log", "message": message})
}

pub fn license_str(c: LicenseClass) -> &'static str {
    match c {
        LicenseClass::Permissive => "permissive",
        LicenseClass::Os => "os",
    }
}

pub fn voice_json(v: &Voice) -> Value {
    json!({
        "id": v.id,
        "name": v.name,
        "language": v.language,
        "engine": v.engine.as_str(),
        "license_class": license_str(v.license_class),
        "installed": v.installed,
    })
}

pub fn setting_to_json(v: &SettingValue) -> Value {
    match v {
        SettingValue::Number(n) => json!(n),
        SettingValue::Text(t) => json!(t),
        SettingValue::Null => Value::Null,
    }
}

/// A JSON `set` value as a facade value: a non-negative integer, a string or
/// null.
pub fn setting_from_json(v: &Value) -> Option<SettingValue> {
    match v {
        Value::Null => Some(SettingValue::Null),
        Value::String(s) => Some(SettingValue::Text(s.clone())),
        Value::Number(n) => n.as_u64().map(SettingValue::Number),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_echo_the_id_only_when_given() {
        let r = ok_reply(Some(&json!(7)), Map::new());
        assert_eq!(r, json!({"id": 7, "ok": true}));
        let r = error_reply(None, &Failure::new(Code::Busy, "busy"));
        assert_eq!(
            r,
            json!({"ok": false, "error": {"code": "E_BUSY", "message": "busy"}})
        );
    }

    #[test]
    fn set_values_accept_integers_strings_and_null_only() {
        assert_eq!(
            setting_from_json(&json!(50)),
            Some(SettingValue::Number(50))
        );
        assert_eq!(
            setting_from_json(&json!("tone")),
            Some(SettingValue::Text("tone".into()))
        );
        assert_eq!(setting_from_json(&Value::Null), Some(SettingValue::Null));
        assert_eq!(setting_from_json(&json!(-1)), None);
        assert_eq!(setting_from_json(&json!(1.5)), None);
        assert_eq!(setting_from_json(&json!(true)), None);
    }
}
