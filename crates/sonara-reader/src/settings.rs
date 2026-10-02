//! The settings `set`/`get` handle (protocol v1 keys) and their checks.
use crate::{Error, Result};
use sonara_engine::Engine;
use std::fmt;

/// Words per minute, as the Python reader (`config_schema.RATE_MIN/MAX`).
pub const RATE_MIN: u32 = 100;
pub const RATE_MAX: u32 = 400;
/// Percent.
pub const VOLUME_MAX: u8 = 100;

/// A setting (spec 4.1 `set`/`get`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Volume,
    Rate,
    Voice,
    Engine,
}

impl Key {
    pub fn as_str(&self) -> &'static str {
        match self {
            Key::Volume => "volume",
            Key::Rate => "rate",
            Key::Voice => "voice",
            Key::Engine => "engine",
        }
    }

    /// The key for a protocol name, if there is one.
    pub fn parse(name: &str) -> Option<Key> {
        [Key::Volume, Key::Rate, Key::Voice, Key::Engine]
            .into_iter()
            .find(|k| k.as_str() == name)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A setting value, shaped like the JSON a protocol client sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Number(u64),
    Text(String),
    Null,
}

fn bad(key: Key, reason: impl Into<String>) -> Error {
    Error::BadValue {
        key,
        reason: reason.into(),
    }
}

pub(crate) fn check_volume(v: u64) -> Result<u8> {
    if v > VOLUME_MAX as u64 {
        return Err(bad(Key::Volume, format!("{v} is not in 0..={VOLUME_MAX}")));
    }
    Ok(v as u8)
}

pub(crate) fn check_rate(v: u32) -> Result<u32> {
    if !(RATE_MIN..=RATE_MAX).contains(&v) {
        return Err(bad(
            Key::Rate,
            format!("{v} is not in {RATE_MIN}..={RATE_MAX}"),
        ));
    }
    Ok(v)
}

pub(crate) fn number(key: Key, value: &Value) -> Result<u64> {
    match value {
        Value::Number(n) => Ok(*n),
        other => Err(bad(key, format!("expected a number, got {other:?}"))),
    }
}

/// The voice id for `voice` (an id or a display name of `engine`), `None`
/// for the default (`None` or empty).
pub(crate) fn resolve_voice(engine: &dyn Engine, voice: Option<&str>) -> Result<Option<String>> {
    let wanted = match voice {
        None | Some("") => return Ok(None),
        Some(v) => v,
    };
    engine
        .voices()
        .into_iter()
        .find(|v| v.id == wanted || v.name == wanted)
        .map(|v| Some(v.id))
        .ok_or_else(|| sonara_engine::Error::UnknownVoice(wanted.to_string()).into())
}

/// True when `engine` offers the voice id `voice`.
pub(crate) fn offers(engine: &dyn Engine, voice: &str) -> bool {
    engine.voices().iter().any(|v| v.id == voice)
}
