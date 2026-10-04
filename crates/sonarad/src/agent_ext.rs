//! The `agent` extension (spec 4.3) on top of `sonara_agent`: `stream`,
//! `turn_start`, `turn_end`, `ask`, `earcon`, `tool`, `answered`, the
//! settings `mute_level`, `verbosity`, `read_mode`, `minqueue`,
//! `background_policy`, `flush_scope` and
//! `summaries`, the read-only key `earcons` (the custom earcons folder and
//! the kinds it overrides), and the `earcons` event stream. It needs
//! `channels`, which enabling it enables too. `protocol` calls in here once a client enabled it; before that its
//! messages are `E_UNSUPPORTED`.
use crate::config::Store;
use crate::protocol::{bad, opt_str, reader_failure, After, Handled};
use crate::wire::{Code, Failure};
use serde_json::{json, Map, Value};
use sonara_agent::settings::{
    BackgroundPolicy, FlushScope, ReadMode, Settings, SummaryCommand, Verbosity,
};
use sonara_agent::summarizer::instruction;
use sonara_agent::{Agent, Ask, AskKind, Choice, Earcon, Error, Style, SummarySettings};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

/// The extension's state: empty until a client enables it.
pub type Slot = Arc<OnceLock<Agent>>;

pub const NAME: &str = "agent";

/// Message types of the extension.
pub const TYPES: &[&str] = &[
    "stream",
    "turn_start",
    "turn_end",
    "ask",
    "earcon",
    "tool",
    "answered",
];

/// `set`/`get` keys of the extension.
pub const KEYS: &[&str] = &[
    "mute_level",
    "verbosity",
    "read_mode",
    "minqueue",
    "background_policy",
    "flush_scope",
    "summaries",
    "earcons",
];

pub(crate) fn failure(e: Error) -> Failure {
    match e {
        Error::Channels(sonara_channels::Error::UnknownChannel(_)) => {
            Failure::new(Code::NotFound, e.to_string())
        }
        Error::Channels(sonara_channels::Error::EmptyChannel) => {
            Failure::new(Code::BadRequest, e.to_string())
        }
        Error::Channels(sonara_channels::Error::Reader(r)) => reader_failure(r),
        Error::BadValue(_) => Failure::new(Code::BadRequest, e.to_string()),
        Error::NoSummarizer => Failure::new(Code::Unsupported, e.to_string()),
    }
}

fn ok(fields: Map<String, Value>) -> Handled {
    Ok((fields, After::Nothing))
}

fn channel(m: &Map<String, Value>) -> Result<&str, Failure> {
    match opt_str(m, "channel")? {
        Some(c) if !c.is_empty() => Ok(c),
        _ => Err(bad("'channel' must be a non-empty string")),
    }
}

/// `t`: the sender's start time in seconds (any number), optional.
fn opt_t(m: &Map<String, Value>) -> Result<Option<f64>, Failure> {
    match m.get("t") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_f64()
            .filter(|f| f.is_finite())
            .map(Some)
            .ok_or_else(|| bad("'t' must be a number")),
        Some(_) => Err(bad("'t' must be a number")),
    }
}

fn opt_flag(m: &Map<String, Value>, field: &str) -> Result<bool, Failure> {
    match m.get(field) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(bad(format!("'{field}' must be true or false"))),
    }
}

fn stale(applied: bool) -> Handled {
    let mut f = Map::new();
    f.insert("stale".into(), json!(!applied));
    ok(f)
}

pub fn stream(a: &Agent, m: &Map<String, Value>) -> Handled {
    let ch = channel(m)?;
    let delta = match m.get("delta") {
        Some(Value::String(d)) => d.as_str(),
        _ => return Err(bad("'delta' must be a string")),
    };
    let index = match m.get("index") {
        None | Some(Value::Null) => 0,
        Some(v) => v
            .as_u64()
            .and_then(|i| u32::try_from(i).ok())
            .ok_or_else(|| bad("'index' must be a non-negative integer"))?,
    };
    let is_final = opt_flag(m, "final")?;
    let turn = opt_str(m, "turn")?;
    let applied = a
        .stream(ch, turn, delta, index, is_final, opt_t(m)?)
        .map_err(failure)?;
    stale(applied)
}

pub fn turn_start(a: &Agent, m: &Map<String, Value>) -> Handled {
    let ch = channel(m)?;
    let applied = a
        .turn_start(ch, opt_str(m, "turn")?, opt_t(m)?)
        .map_err(failure)?;
    stale(applied)
}

pub fn turn_end(a: &Agent, m: &Map<String, Value>) -> Handled {
    let ch = channel(m)?;
    let applied = a
        .turn_end(ch, opt_str(m, "turn")?, opt_t(m)?)
        .map_err(failure)?;
    stale(applied)
}

/// `options`: strings or `{label, description?}`.
fn options(m: &Map<String, Value>) -> Result<Vec<Choice>, Failure> {
    let err = || bad("'options' must be a list of strings or {label, description} objects");
    match m.get("options") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(list)) => list
            .iter()
            .map(|o| match o {
                Value::String(s) => Ok(Choice {
                    label: s.clone(),
                    description: None,
                }),
                Value::Object(o) => Ok(Choice {
                    label: opt_str(o, "label")
                        .map_err(|_| err())?
                        .unwrap_or("")
                        .to_string(),
                    description: opt_str(o, "description")
                        .map_err(|_| err())?
                        .map(str::to_string),
                }),
                _ => Err(err()),
            })
            .collect(),
        Some(_) => Err(err()),
    }
}

pub fn ask(a: &Agent, m: &Map<String, Value>) -> Handled {
    let ch = channel(m)?;
    let kind = opt_str(m, "kind")?.ok_or_else(|| bad("missing 'kind'"))?;
    let kind = AskKind::parse(kind).ok_or_else(|| {
        bad(format!(
            "unknown ask kind '{kind}' (question, permission or plan)"
        ))
    })?;
    let owned = |f: &str| opt_str(m, f).map(|o| o.map(str::to_string));
    let mut ask = Ask::new(kind, opt_str(m, "text")?.unwrap_or(""));
    ask.options = options(m)?;
    ask.multi = opt_flag(m, "multi_select")?;
    ask.notes = owned("notes")?;
    ask.hint = owned("hint")?;
    ask.hint_once = owned("hint_once")?;
    a.ask(ch, &ask).map_err(failure)?;
    ok(Map::new())
}

/// The support log line of an accepted `ask`: its kind and session (the
/// channel's label, else its id). Never the question's text.
pub fn ask_line(a: &Agent, m: &Map<String, Value>) -> String {
    let id = m.get("channel").and_then(Value::as_str).unwrap_or("");
    let label = a.channels().channel(id).and_then(|c| c.label);
    let kind = m.get("kind").and_then(Value::as_str).unwrap_or("");
    let session = label
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| id.to_string());
    crate::support_log::ask_line(kind, &session)
}

pub fn earcon(a: &Agent, m: &Map<String, Value>) -> Handled {
    let kind = opt_str(m, "kind")?.ok_or_else(|| bad("missing 'kind'"))?;
    let e = Earcon::parse(kind).ok_or_else(|| bad(format!("unknown earcon '{kind}'")))?;
    a.earcon(e).map_err(failure)?;
    ok(Map::new())
}

pub fn tool(a: &Agent, m: &Map<String, Value>) -> Handled {
    let ch = channel(m)?;
    let name = opt_str(m, "name")?.unwrap_or("");
    let summary = opt_str(m, "summary")?.unwrap_or("");
    a.tool(ch, name, summary).map_err(failure)?;
    ok(Map::new())
}

pub fn answered(a: &Agent, m: &Map<String, Value>) -> Handled {
    a.answered(channel(m)?).map_err(failure)?;
    ok(Map::new())
}

/// `channel_close` once the extension is on: the agent forgets the turn.
pub fn close(a: &Agent, m: &Map<String, Value>) -> Handled {
    a.close(channel(m)?).map_err(failure)?;
    ok(Map::new())
}

const STYLES: [Style; 3] = [Style::Tidy, Style::Natural, Style::Brief];

fn summaries_json(s: &SummarySettings, prompts: &BTreeMap<String, String>) -> Value {
    let defaults: Map<String, Value> = STYLES
        .iter()
        .map(|st| (st.as_str().to_string(), json!(instruction(*st))))
        .collect();
    json!({
        "enabled": s.enabled,
        "command": s.command.as_str(),
        "model": s.model,
        "timeout": s.timeout_s,
        "settle_ms": s.settle_ms,
        "style": s.style.as_str(),
        "prompt": s.prompt,
        "prompts": prompts,
        "default_prompts": defaults,
    })
}

/// Merge the fields given in `v` into `s` (not the prompts: see
/// `merge_prompts`).
fn merge_summaries(mut s: SummarySettings, v: &Value) -> Result<SummarySettings, Failure> {
    let Value::Object(o) = v else {
        return Err(bad("'summaries' is an object"));
    };
    if let Some(e) = o.get("enabled") {
        s.enabled = e
            .as_bool()
            .ok_or_else(|| bad("'summaries.enabled' must be true or false"))?;
    }
    if let Some(c) = opt_str(o, "command")? {
        s.command = SummaryCommand::parse(c)
            .ok_or_else(|| bad("'summaries.command' is \"claude\" or \"codex\""))?;
    }
    if let Some(model) = opt_str(o, "model")? {
        s.model = model.to_string();
    }
    if let Some(t) = o.get("timeout").filter(|t| !t.is_null()) {
        s.timeout_s = t
            .as_u64()
            .ok_or_else(|| bad("'summaries.timeout' must be whole seconds"))?;
    }
    if let Some(t) = o.get("settle_ms").filter(|t| !t.is_null()) {
        s.settle_ms = t
            .as_u64()
            .ok_or_else(|| bad("'summaries.settle_ms' must be whole milliseconds"))?;
    }
    if let Some(st) = opt_str(o, "style")? {
        s.style = Style::parse(st)
            .ok_or_else(|| bad("'summaries.style' is \"tidy\", \"natural\" or \"brief\""))?;
    }
    Ok(s)
}

/// The custom prompts after `v`: `prompts` sets (a string) or resets
/// (`null` or blank) each style given; `prompt` does the same for the
/// style in force after this request.
fn merge_prompts(
    mut prompts: BTreeMap<String, String>,
    v: &Value,
    style: Style,
) -> Result<BTreeMap<String, String>, Failure> {
    let mut apply = |style: &str, text: &Value| -> Result<(), Failure> {
        match text {
            Value::Null => {
                prompts.remove(style);
            }
            Value::String(t) if t.trim().is_empty() => {
                prompts.remove(style);
            }
            Value::String(t) => {
                prompts.insert(style.to_string(), t.clone());
            }
            _ => return Err(bad("a summary prompt is a string or null")),
        }
        Ok(())
    };
    if let Some(p) = v.get("prompts").filter(|p| !p.is_null()) {
        let o = p
            .as_object()
            .ok_or_else(|| bad("'summaries.prompts' is an object of style: prompt"))?;
        for (st, text) in o {
            if Style::parse(st).is_none() {
                return Err(bad(format!(
                    "'summaries.prompts' has an unknown style '{st}'"
                )));
            }
            apply(st, text)?;
        }
    }
    if let Some(text) = v.get("prompt") {
        apply(style.as_str(), text)?;
    }
    Ok(prompts)
}

/// The agent's settings from the persisted ones (applied when a client
/// enables the extension). Stored values were validated when loaded.
pub fn settings_from(store: &Store) -> Settings {
    let mut s = Settings::default();
    if let Some(n) = store.value("mute_level").as_u64() {
        s.mute_level = n as u8;
    }
    if let Some(v) = store.value("verbosity").as_str().and_then(Verbosity::parse) {
        s.verbosity = v;
    }
    if let Some(m) = store.value("read_mode").as_str().and_then(ReadMode::parse) {
        s.read_mode = m;
    }
    if let Some(n) = store.value("minqueue").as_u64() {
        s.minqueue = n as usize;
    }
    if let Some(p) = store
        .value("background_policy")
        .as_str()
        .and_then(BackgroundPolicy::parse)
    {
        s.background = p;
    }
    if let Some(f) = store
        .value("flush_scope")
        .as_str()
        .and_then(FlushScope::parse)
    {
        s.flush_scope = f;
    }
    if let Ok(merged) = merge_summaries(s.summaries.clone(), &Value::Object(store.summaries())) {
        s.summaries = merged;
    }
    s.summaries.prompt = store.prompts().get(s.summaries.style.as_str()).cloned();
    s
}

/// `set`/`get` of an extension key (`value` is `None` for `get`). A `set`
/// that succeeds is persisted (`summaries`: only the fields given, and the
/// custom prompts).
pub fn setting(a: &Agent, store: &Store, key: &str, value: Option<&Value>) -> Handled {
    if let Some(v) = value {
        match key {
            "earcons" => return Err(bad("'earcons' is read-only")),
            "mute_level" => {
                let n = v
                    .as_u64()
                    .filter(|n| *n <= 2)
                    .ok_or_else(|| bad("'mute_level' is 0, 1 or 2"))?;
                a.set_mute_level(n as u8).map_err(failure)?;
            }
            "verbosity" => {
                let name = v.as_str().unwrap_or("");
                let vb = Verbosity::parse(name)
                    .ok_or_else(|| bad("'verbosity' is \"everything\" or \"skip_code\""))?;
                a.set_verbosity(vb);
            }
            "read_mode" => {
                let m = v
                    .as_str()
                    .and_then(ReadMode::parse)
                    .ok_or_else(|| bad("'read_mode' is \"immediate\", \"queue\" or \"done\""))?;
                a.set_read_mode(m);
            }
            "minqueue" => {
                let n = v
                    .as_u64()
                    .ok_or_else(|| bad("'minqueue' must be a non-negative integer"))?;
                a.set_minqueue(n as usize).map_err(failure)?;
            }
            "background_policy" => {
                let p = v
                    .as_str()
                    .and_then(BackgroundPolicy::parse)
                    .ok_or_else(|| bad("'background_policy' is \"all\" or \"earcon_only\""))?;
                a.set_background_policy(p).map_err(failure)?;
            }
            "flush_scope" => {
                let f = v
                    .as_str()
                    .and_then(FlushScope::parse)
                    .ok_or_else(|| bad("'flush_scope' is \"session\" or \"all\""))?;
                a.set_flush_scope(f);
            }
            _ => {
                let mut merged = merge_summaries(a.settings().summaries, v)?;
                let prompts = merge_prompts(store.prompts(), v, merged.style)?;
                merged.prompt = prompts.get(merged.style.as_str()).cloned();
                a.set_summaries(merged).map_err(failure)?;
                // Persist the fields given (valid now) and the prompts.
                let mut fields: Map<String, Value> = v
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .filter(|(k, val)| !val.is_null() && *k != "prompt" && *k != "prompts")
                            .map(|(k, val)| (k.clone(), val.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                if v.get("prompt").is_some() || v.get("prompts").is_some() {
                    fields.insert("prompts".into(), json!(prompts));
                }
                store.record_summaries(&fields);
            }
        }
    }
    let s = a.settings();
    let value = match key {
        "mute_level" => json!(s.mute_level),
        "verbosity" => json!(s.verbosity.as_str()),
        "read_mode" => json!(s.read_mode.as_str()),
        "minqueue" => json!(s.minqueue),
        "background_policy" => json!(s.background.as_str()),
        "flush_scope" => json!(s.flush_scope.as_str()),
        "earcons" => earcons_json(a.earcons()),
        _ => summaries_json(&s.summaries, &store.prompts()),
    };
    let mut f = Map::new();
    f.insert("key".into(), json!(key));
    f.insert("value".into(), value);
    ok(f)
}

/// `get earcons`: the folder whose `<kind>.wav` files replace the bundled
/// clips (`null`: none), every kind, and the kinds a usable file replaces
/// now.
pub fn earcons_json(lib: &sonara_agent::Library) -> Value {
    json!({
        "folder": lib.dir().map(|d| d.display().to_string()),
        "kinds": Earcon::ALL.iter().map(Earcon::as_str).collect::<Vec<_>>(),
        "custom": lib.custom().iter().map(Earcon::as_str).collect::<Vec<_>>(),
    })
}

/// The `earcon` event.
pub fn earcon_event(e: Earcon) -> Value {
    json!({"event": "earcon", "kind": e.as_str()})
}

#[cfg(test)]
mod tests {
    use crate::lifetime::Lifetime;
    use crate::protocol::{Server, Session};
    use serde_json::{json, Value};
    use sonara_audio::TestOutput;
    use sonara_engine::fake::FakeEngine;
    use sonara_reader::{Config, ReaderHandle, Registry};
    use std::sync::Arc;
    use std::time::Duration;

    fn server() -> Server {
        let mut registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap();
        Server::new(
            reader,
            "secret".into(),
            Lifetime::new(Duration::from_secs(30), false),
        )
    }

    fn call(s: &Server, session: &mut Session, req: Value) -> Value {
        s.handle(session, &req).reply
    }

    fn code(r: &Value) -> &str {
        r["error"]["code"].as_str().unwrap_or("")
    }

    fn enabled() -> (Server, Session) {
        let s = server();
        let mut a = Session::tcp();
        let r = call(
            &s,
            &mut a,
            json!({"type": "hello", "token": "secret", "extensions": ["agent"]}),
        );
        assert_eq!(r["extensions"], json!(["channels", "agent"]), "{r}");
        (s, a)
    }

    #[test]
    fn the_extension_is_unsupported_until_enabled_and_brings_channels() {
        let s = server();
        let mut a = Session::tcp();
        call(&s, &mut a, json!({"type": "hello", "token": "secret"}));
        for req in [
            json!({"type": "stream", "channel": "a", "delta": "x"}),
            json!({"type": "tool", "channel": "a", "name": "Bash"}),
            json!({"type": "get", "key": "mute_level"}),
            json!({"type": "subscribe", "events": ["earcons"]}),
        ] {
            assert_eq!(
                code(&call(&s, &mut a, req.clone())),
                "E_UNSUPPORTED",
                "{req}"
            );
        }
        let mut b = Session::tcp();
        call(
            &s,
            &mut b,
            json!({"type": "hello", "token": "secret", "require": ["agent"]}),
        );
        assert!(s.channels().is_some() && s.agent().is_some());
        let r = call(
            &s,
            &mut a,
            json!({"type": "stream", "channel": "a", "delta": "Hi.", "final": true}),
        );
        assert_eq!(r["stale"], false, "{r}");
    }

    #[test]
    fn agent_messages_check_their_fields() {
        let (s, mut a) = enabled();
        let cases = [
            json!({"type": "stream", "channel": "a"}),
            json!({"type": "stream", "channel": "", "delta": "x"}),
            json!({"type": "stream", "channel": "a", "delta": "x", "index": -1}),
            json!({"type": "stream", "channel": "a", "delta": "x", "t": "soon"}),
            json!({"type": "stream", "channel": "a", "delta": "x", "final": 1}),
            json!({"type": "turn_start"}),
            json!({"type": "ask", "channel": "a"}),
            json!({"type": "ask", "channel": "a", "kind": "quiz"}),
            json!({"type": "ask", "channel": "a", "kind": "question", "options": [1]}),
            json!({"type": "earcon", "kind": "ready"}),
            json!({"type": "earcon"}),
            json!({"type": "answered"}),
            json!({"type": "set", "key": "mute_level", "value": 3}),
            json!({"type": "set", "key": "verbosity", "value": "loud"}),
            json!({"type": "set", "key": "minqueue", "value": 11}),
            json!({"type": "set", "key": "read_mode", "value": "later"}),
            json!({"type": "set", "key": "read_mode", "value": 2}),
            json!({"type": "set", "key": "flush_scope", "value": "everything"}),
            json!({"type": "set", "key": "flush_scope", "value": true}),
            json!({"type": "set", "key": "background_policy", "value": "silent"}),
            json!({"type": "set", "key": "summaries", "value": {"timeout": 5}}),
            json!({"type": "set", "key": "summaries", "value": {"style": "long"}}),
            json!({"type": "set", "key": "summaries", "value": 1}),
        ];
        for req in cases {
            assert_eq!(
                code(&call(&s, &mut a, req.clone())),
                "E_BAD_REQUEST",
                "{req}"
            );
        }
    }

    #[test]
    fn late_text_is_reported_stale() {
        let (s, mut a) = enabled();
        let r = call(
            &s,
            &mut a,
            json!({"type": "turn_start", "channel": "a", "t": 10.5}),
        );
        assert_eq!(r["stale"], false);
        let r = call(
            &s,
            &mut a,
            json!({"type": "stream", "channel": "a", "delta": "Old.", "t": 9}),
        );
        assert_eq!(r["stale"], true);
        let r = call(
            &s,
            &mut a,
            json!({"type": "turn_end", "channel": "a", "t": 9.9}),
        );
        assert_eq!(r["stale"], true);
    }

    #[test]
    fn settings_round_trip() {
        let (s, mut a) = enabled();
        let r = call(&s, &mut a, json!({"type": "get", "key": "summaries"}));
        let mut v = r["value"].clone();
        let defaults = v
            .as_object_mut()
            .unwrap()
            .remove("default_prompts")
            .unwrap();
        assert_eq!(
            v,
            json!({"enabled": false, "command": "claude", "model": "haiku", "timeout": 60,
                   "settle_ms": 600, "style": "natural", "prompt": null, "prompts": {}})
        );
        for st in ["tidy", "natural", "brief"] {
            assert!(!defaults[st].as_str().unwrap().is_empty(), "{st}");
        }
        let r = call(
            &s,
            &mut a,
            json!({"type": "set", "key": "summaries",
                   "value": {"style": "brief", "timeout": 30, "prompt": "Short."}}),
        );
        assert_eq!(r["value"]["style"], "brief");
        assert_eq!(r["value"]["timeout"], 30);
        assert_eq!(r["value"]["model"], "haiku", "merged, not replaced");
        assert_eq!(r["value"]["prompt"], "Short.");
        assert_eq!(r["value"]["prompts"], json!({"brief": "Short."}));
        // Each style keeps its own prompt; the one in force follows the style.
        let r = call(
            &s,
            &mut a,
            json!({"type": "set", "key": "summaries",
                   "value": {"style": "tidy", "prompts": {"tidy": "All of it."}}}),
        );
        assert_eq!(r["value"]["prompt"], "All of it.");
        assert_eq!(
            r["value"]["prompts"],
            json!({"brief": "Short.", "tidy": "All of it."})
        );
        let r = call(
            &s,
            &mut a,
            json!({"type": "set", "key": "summaries", "value": {"style": "natural"}}),
        );
        assert_eq!(
            r["value"]["prompt"],
            Value::Null,
            "natural has no custom prompt"
        );
        let r = call(
            &s,
            &mut a,
            json!({"type": "set", "key": "summaries",
                   "value": {"prompts": {"brief": null, "tidy": "  "}}}),
        );
        assert_eq!(r["value"]["prompts"], json!({}), "null or blank resets");
        let r = call(
            &s,
            &mut a,
            json!({"type": "set", "key": "summaries", "value": {"prompts": {"poem": "x"}}}),
        );
        assert_eq!(code(&r), "E_BAD_REQUEST");
        for (key, value) in [
            ("mute_level", json!(2)),
            ("verbosity", json!("skip_code")),
            ("verbosity", json!("everything")),
            ("minqueue", json!(3)),
            ("read_mode", json!("immediate")),
            ("read_mode", json!("queue")),
            ("read_mode", json!("done")),
            ("flush_scope", json!("all")),
            ("flush_scope", json!("session")),
            ("background_policy", json!("all")),
            ("background_policy", json!("earcon_only")),
        ] {
            let r = call(
                &s,
                &mut a,
                json!({"type": "set", "key": key, "value": value}),
            );
            assert_eq!(r["value"], value, "{key}");
            let r = call(&s, &mut a, json!({"type": "get", "key": key}));
            assert_eq!(r["value"], value, "{key}");
        }
    }

    #[test]
    fn old_verbosity_names_are_aliases_that_answer_the_new_name() {
        // #214: older clients still send the three-level names.
        let (s, mut a) = enabled();
        for (old, now) in [
            ("medium", "skip_code"),
            ("quiet", "skip_code"),
            ("all", "everything"),
        ] {
            let r = call(
                &s,
                &mut a,
                json!({"type": "set", "key": "verbosity", "value": old}),
            );
            assert_eq!(r["value"], now, "{old}");
        }
    }

    #[test]
    fn ask_tool_answered_earcon_and_close() {
        let (s, mut a) = enabled();
        for req in [
            json!({"type": "ask", "channel": "a", "kind": "question", "text": "Pick?",
                   "options": ["A", {"label": "B", "description": "second"}],
                   "multi_select": true, "notes": "n", "hint": "h", "hint_once": "o"}),
            json!({"type": "ask", "channel": "a", "kind": "permission", "text": "Run ls"}),
            json!({"type": "tool", "channel": "a", "name": "Bash", "summary": "ls"}),
            json!({"type": "answered", "channel": "a"}),
            json!({"type": "earcon", "kind": "nav_edge"}),
            json!({"type": "channel_close", "channel": "a"}),
        ] {
            let r = call(&s, &mut a, req.clone());
            assert_eq!(r["ok"], true, "{req} -> {r}");
        }
        let r = call(&s, &mut a, json!({"type": "channel_close", "channel": "a"}));
        assert_eq!(code(&r), "E_NOT_FOUND");
    }
}
