//! Protocol handlers of the external engine messages (spec 10.2):
//! `engine_list`, `engine_add`, `engine_key`, `engine_test`, and the
//! `refresh` of `voices`. `engine_remove` is in `protocol.rs`, which owns
//! `set engine` (the current engine is switched away first).
use crate::cues;
use crate::engines::Engines;
use crate::protocol::{bad, opt_str, reader_failure, After, Handled};
use crate::wire::{self, Code, Failure};
use serde_json::{json, Map, Value};
use sonara_engine::Error as EngineError;
use sonara_reader::{Key, ReaderHandle};

/// What `engine_test` says when no text is given.
pub const TEST_TEXT: &str = "Hello. This is how Sonara sounds with this voice.";
/// The longest `engine_test` text.
pub const TEST_MAX: usize = 300;

/// The refusal when the host does not allow external engines.
pub fn refused() -> Failure {
    Failure::new(
        Code::Unsupported,
        "this runtime does not allow external engines",
    )
}

/// An engine failure as a reply, with `reason` for an external engine's.
pub fn engine_failure(e: EngineError) -> Failure {
    match e {
        EngineError::External { reason, message } => {
            Failure::new(Code::Engine, message).with_reason(reason.as_str())
        }
        other => reader_failure(sonara_reader::Error::Engine(other)),
    }
}

fn required<'a>(m: &'a Map<String, Value>, field: &str) -> Result<&'a str, Failure> {
    opt_str(m, field)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad(format!("missing '{field}'")))
}

fn flag(m: &Map<String, Value>, field: &str, default: bool) -> Result<bool, Failure> {
    match m.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(bad(format!("'{field}' must be true or false"))),
    }
}

/// `engine_list`.
pub fn list(engines: &Engines, current: &str) -> Handled {
    Ok((engines.list(current), After::Nothing))
}

/// `engine_add` `{engine, secret?, replace?}`. A replaced current engine
/// applies to the next chunk.
pub fn add(
    engines: &Engines,
    reader: &ReaderHandle,
    m: &Map<String, Value>,
    current: &str,
) -> Handled {
    let engine = m.get("engine").ok_or_else(|| bad("missing 'engine'"))?;
    let secret = match m.get("secret") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return Err(bad("'secret' must be a string")),
    };
    let replace = flag(m, "replace", false)?;
    engines.add(engine, secret, replace)?;
    let id = engine
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if id == current {
        // The same id again swaps the reader to the new instance.
        reader
            .set(Key::Engine, sonara_reader::Value::Text(id.clone()))
            .map_err(reader_failure)?;
    }
    let mut f = Map::new();
    f.insert(
        "engine".into(),
        engines.view_of(&id, current).unwrap_or(Value::Null),
    );
    Ok((f, After::Nothing))
}

/// `engine_key` `{engine, secret: string | null}`. The profile is named by
/// `engine`: a request's `id` is its correlation id.
pub fn key(engines: &Engines, m: &Map<String, Value>) -> Handled {
    let id = required(m, "engine")?;
    let secret = match m.get("secret") {
        Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.as_str()),
        None => {
            return Err(bad(
                "missing 'secret' (a string, or null to delete the key)",
            ))
        }
        Some(_) => return Err(bad("'secret' must be a string or null")),
    };
    let present = engines.set_key(id, secret)?;
    let mut f = Map::new();
    f.insert("engine".into(), json!(id));
    f.insert("key_present".into(), json!(present));
    Ok((f, After::Nothing))
}

/// `engine_test` `{engine, text?, voice?, play?}`: one synthesis with no
/// fallback, at the current rate, played over whatever is read.
///
/// `admit` is taken only to play the clip, after the provider answered:
/// the round trip must not hold up speech or controls.
pub fn test<G>(
    engines: &Engines,
    reader: &ReaderHandle,
    m: &Map<String, Value>,
    admit: impl FnOnce() -> Result<G, Failure>,
) -> Handled {
    let id = required(m, "engine")?;
    let ext = match engines.get(id) {
        Some(x) => x,
        None if engines.contains(id) => {
            return Err(Failure::new(
                Code::Unsupported,
                format!("engine '{id}' cannot be used by this version"),
            ))
        }
        None => return Err(Failure::new(Code::NotFound, format!("no engine '{id}'"))),
    };
    let text = opt_str(m, "text")?
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(TEST_TEXT);
    if text.chars().count() > TEST_MAX {
        return Err(bad(format!("'text' is longer than {TEST_MAX} characters")));
    }
    let voice = opt_str(m, "voice")?.unwrap_or_default();
    let play = flag(m, "play", true)?;
    let rate = match reader.get(Key::Rate).map_err(reader_failure)? {
        sonara_reader::Value::Number(n) => n as u32,
        _ => 200,
    };
    let result = ext.test(text, voice, rate).map_err(engine_failure)?;
    let (samples, sample_rate) = cues::mono(&result.pcm);
    let duration_ms = if sample_rate > 0 {
        samples.len() as u64 * 1000 / sample_rate as u64
    } else {
        0
    };
    if play {
        let _admitted = admit()?;
        reader
            .play_clip(samples, sample_rate)
            .map_err(reader_failure)?;
    }
    let mut f = Map::new();
    f.insert("engine".into(), json!(id));
    f.insert("voice".into(), json!(result.voice));
    f.insert("ms".into(), json!(result.ms));
    f.insert("sample_rate".into(), json!(sample_rate));
    f.insert("duration_ms".into(), json!(duration_ms));
    Ok((f, After::Nothing))
}

/// `voices` for a profile: fetched again when the cache is old or
/// `refresh` is set; a failure keeps the known voices and adds `error`.
pub fn voices(
    engines: &Engines,
    reader: &ReaderHandle,
    engine: &str,
    refresh: bool,
) -> Option<Handled> {
    let ext = engines.get(engine)?;
    let mut f = Map::new();
    let mut error = None;
    if refresh || ext.voices_stale() {
        if let Err(e) = reader.refresh_voices(engine) {
            let failure = match e {
                sonara_reader::Error::Engine(e) => engine_failure(e),
                other => reader_failure(other),
            };
            error = Some(json!({
                "reason": failure.reason.unwrap_or("network"),
                "message": failure.message,
            }));
        }
    }
    let list = match reader.voices(Some(engine)) {
        Ok(v) => v,
        Err(e) => return Some(Err(reader_failure(e))),
    };
    f.insert(
        "voices".into(),
        Value::Array(list.iter().map(wire::voice_json).collect()),
    );
    if let Some(e) = error {
        f.insert("error".into(), e);
    }
    Some(Ok((f, After::Nothing)))
}
