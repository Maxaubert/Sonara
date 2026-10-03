//! The troubleshooting lines of `logs\sonarad.log` (#219), next to the
//! activity lines of `support_log`, so "why was X (not) read" can be
//! answered from the log alone:
//!
//! - `in {json}`: every protocol message received, before it is handled
//!   (but the read-only `get` and `voices`, and the settings page's
//!   `hello` polls), compact: the token is never written, `options` become
//!   their labels and a string over `FIELD_MAX` is clipped; `in failed
//!   type=<t> E_...: <message>` when it was refused.
//! - `agent <source> channel=<id> ...`: what the agent did with it
//!   (`sonara_agent::Trace`): `speak kind=<k> entry=<n>` with the text
//!   added to the channel (and `waits=` when a muted session or the
//!   background policy holds it), a note when the rules spoke nothing and
//!   why, `dropped: late text ...`, `earcon <name>`, `wipe reason=<r>`.
//! - `drop channel=<id> entry=<n> kind=<k> reason=<r>`: text dropped
//!   before it was heard (`sonara_channels::Dropped`), `item=<id>` when it
//!   was cut while being read.
//! - `cue text=...`: a spoken control cue ("Paused.").
//! - `read text item=<id> ...` (in `support_log`): what went to the voice.
//!
//! **Privacy.** With the setting `debug_log` off (`set_debug(false)`) no
//! text, payload or option label is written: the lines keep their fixed
//! fields only. It is on by default for now (the user's choice, #219).
use serde_json::{json, Map, Value};
use sonara_agent::{Trace, Traced};
use sonara_channels::Dropped;
use sonara_system::log::value;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A protocol string field longer than this (bytes) is clipped in an `in`
/// line.
pub const FIELD_MAX: usize = 4096;
/// Entries whose origin is remembered (`Origins`).
const ORIGINS: usize = 1024;

/// The setting (`config.json`, protocol `set`/`get`).
pub const DEBUG_KEY: &str = "debug_log";

static DEBUG: AtomicBool = AtomicBool::new(true);

/// Write text and payloads (the `debug_log` setting).
pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::SeqCst);
}

pub fn debug() -> bool {
    DEBUG.load(Ordering::SeqCst)
}

/// ` text="..."` (JSON-escaped), or nothing when `debug` is off.
pub fn text_field(text: &str, debug: bool) -> String {
    if !debug {
        return String::new();
    }
    format!(" text={}", Value::String(text.to_string()))
}

/// What produced a channel entry: its kind (`prose`, `question`, ...)
/// and the message (`stream`, `ask permission`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub kind: String,
    pub source: String,
}

/// The origin of recent channel entries, by entry id: the agent records
/// what it adds, the reading log looks it up when the entry is read.
#[derive(Debug, Clone, Default)]
pub struct Origins(Arc<Mutex<VecDeque<(u64, Origin)>>>);

impl Origins {
    pub fn record(&self, entry: u64, kind: &str, source: &str) {
        let mut q = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if q.len() >= ORIGINS {
            q.pop_front();
        }
        q.push_back((
            entry,
            Origin {
                kind: kind.to_string(),
                source: source.to_string(),
            },
        ));
    }

    pub fn get(&self, entry: u64) -> Option<Origin> {
        let q = self.0.lock().unwrap_or_else(|p| p.into_inner());
        q.iter()
            .rev()
            .find(|(e, _)| *e == entry)
            .map(|(_, o)| o.clone())
    }
}

fn channel_field(channel: Option<&str>) -> String {
    match channel {
        Some(c) => format!(" channel={}", value(c)),
        None => String::new(),
    }
}

/// The line of one agent trace.
pub fn agent_line(t: &Trace, debug: bool) -> String {
    let head = format!(
        "agent {}{}",
        value(&t.source),
        channel_field(t.channel.as_deref())
    );
    match &t.what {
        Traced::Spoken {
            kind,
            entry,
            text,
            decision,
            waits,
        } => format!(
            "{head} speak kind={kind} entry={entry}{}{}{}",
            if *decision { " decision" } else { "" },
            waits
                .map(|w| format!(" waits={}", value(w)))
                .unwrap_or_default(),
            text_field(text, debug)
        ),
        Traced::Note(n) => format!(
            "{head} {}: {}{}",
            n.kind,
            n.what,
            n.text
                .as_deref()
                .map(|t| text_field(t, debug))
                .unwrap_or_default()
        ),
        Traced::Late => format!(
            "{head} dropped: late text of an earlier turn (stamped before the session's \
             last turn_start, #174)"
        ),
        Traced::Earcon(e) => format!("{head} earcon {}", e.as_str()),
        Traced::Wiped { reason } => format!("{head} wipe reason={reason}"),
    }
}

/// The line of text dropped before it was heard.
pub fn drop_line(d: &Dropped, origin: Option<&Origin>, debug: bool) -> String {
    format!(
        "drop channel={} entry={}{}{} reason={}{}",
        value(&d.channel),
        d.entry,
        d.item
            .map(|i| format!(" item={} (cut while read)", i.0))
            .unwrap_or_default(),
        origin
            .map(|o| format!(" kind={} from={}", o.kind, value(&o.source)))
            .unwrap_or_default(),
        value(&d.reason),
        text_field(&d.text, debug)
    )
}

/// The line of a spoken control cue.
pub fn cue_line(text: &str) -> String {
    // A cue is the runtime's own words ("Paused."), never the user's text.
    format!("cue text={}", Value::String(text.to_string()))
}

/// Fields kept in an `in` line when `debug_log` is off: no text.
const PRIVATE_FIELDS: &[&str] = &[
    "type",
    "id",
    "channel",
    "kind",
    "t",
    "index",
    "final",
    "turn",
    "key",
    "action",
    "name",
    "mode",
    "interrupt",
    "multi_select",
    "policy",
    "client",
    "extensions",
    "require",
    "keep_alive",
    "takeover",
    "events",
];

/// Make `v` fit for the log, anywhere in it: the value of a field whose
/// name says it holds a secret is masked, credential-looking words in text
/// are masked (`sonara_log::mask`), and every string over `FIELD_MAX`
/// bytes is clipped.
pub fn scrub(v: &mut Value) {
    match v {
        Value::String(s) => {
            if let std::borrow::Cow::Owned(m) = sonara_log::mask(s) {
                *s = m;
            }
            if s.len() > FIELD_MAX {
                *s = sonara_log::clip(s, FIELD_MAX).into_owned();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(scrub),
        Value::Object(o) => {
            for (k, v) in o.iter_mut() {
                if sonara_log::secret_key(k) && !v.is_null() {
                    *v = Value::String(sonara_log::MASK.into());
                } else {
                    scrub(v);
                }
            }
        }
        _ => {}
    }
}

/// Whether the `value` of a `set` is safe to log with `debug_log` off.
fn plain(m: &Map<String, Value>, v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
        Value::String(_) => m
            .get("key")
            .and_then(Value::as_str)
            .is_some_and(|k| k != "summaries" && crate::config::setting(k).is_some()),
        _ => false,
    }
}

/// A message as logged (module docs), or `None` for one not logged.
pub fn input_json(m: &Map<String, Value>, debug: bool) -> Value {
    let mut out = Map::new();
    for (k, v) in m {
        if k == "token" {
            continue;
        }
        if k == "options" {
            let labels: Vec<Value> = v
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|o| o.get("label").cloned().unwrap_or_else(|| o.clone()))
                        .collect()
                })
                .unwrap_or_default();
            if debug {
                out.insert("options".into(), Value::Array(labels));
            } else {
                out.insert("options".into(), json!(labels.len()));
            }
            continue;
        }
        // A `set` value is kept when it is a number, a switch or a choice
        // of the runtime's own settings (a rate, a voice). Free text is
        // not: `channel_prefs` and `summaries` carry the user's own labels
        // and prompts, an extension setting may be a folder path.
        let keep = debug || PRIVATE_FIELDS.contains(&k.as_str()) || (k == "value" && plain(m, v));
        if keep {
            out.insert(k.clone(), v.clone());
        }
    }
    let mut v = Value::Object(out);
    scrub(&mut v);
    v
}

/// Message types never logged as `in`: read-only queries the settings page
/// polls.
const QUIET: &[&str] = &["get", "voices"];

/// Whether a message of type `kind` gets `in` lines: not `QUIET`, nor a
/// `hello` over HTTP (the settings page's poll).
pub fn logged(kind: &str, over_http: bool) -> bool {
    !(QUIET.contains(&kind) || (over_http && kind == "hello"))
}

/// The `in` line of a message, or `None` when it is not logged.
pub fn input_line(m: &Map<String, Value>, over_http: bool, debug: bool) -> Option<String> {
    let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
    logged(kind, over_http).then(|| format!("in {}", input_json(m, debug)))
}

/// Seconds between two logged refusals of a connection that has not
/// said `hello` with the token: anyone on the machine can reach the port,
/// so what they send is never logged and their refusals are rate limited.
const UNAUTHED_EVERY: u64 = 10;
static UNAUTHED: Gate = Gate::new(UNAUTHED_EVERY);

/// Lets one event through per `every` seconds.
pub struct Gate {
    every: u64,
    last: AtomicU64,
}

impl Gate {
    pub const fn new(every: u64) -> Gate {
        Gate {
            every,
            last: AtomicU64::new(0),
        }
    }

    /// Whether an event at `now` (seconds) passes.
    pub fn allow(&self, now: u64) -> bool {
        let last = self.last.load(Ordering::SeqCst);
        (last == 0 || now >= last + self.every)
            && self
                .last
                .compare_exchange(last, now.max(1), Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
    }
}

/// Whether a refusal before `hello` may be logged now (`now` in seconds).
pub fn unauthed_allowed(now: u64) -> bool {
    UNAUTHED.allow(now)
}

/// The line of a refused message.
pub fn failed_line(kind: &str, code: &str, message: &str) -> String {
    format!(
        "in failed type={} {code}: {message}",
        value(&sonara_log::clip(kind, 64))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonara_agent::{Earcon, Note};

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn input_lines_show_the_message_without_the_token() {
        let ask = map(json!({
            "type": "ask", "channel": "c1", "kind": "question", "t": 12.5,
            "text": "Which one?",
            "options": [{"label": "Red", "description": "warm"}, {"label": "Blue"}],
            "hint": "Press a number.",
        }));
        let line = input_line(&ask, false, true).unwrap();
        assert!(line.starts_with("in {") && line.ends_with('}'), "{line}");
        assert!(line.contains(r#""options":["Red","Blue"]"#), "{line}");
        assert!(line.contains(r#""text":"Which one?""#), "{line}");
        let hello = map(json!({"type": "hello", "token": "s3cret", "client": {"name": "x"}}));
        let line = input_line(&hello, false, true).unwrap();
        assert!(!line.contains("s3cret"), "{line}");
        assert!(input_line(&hello, true, true).is_none(), "page polls");
        assert!(input_line(&map(json!({"type": "get", "key": "rate"})), false, true).is_none());
        assert_eq!(
            failed_line("ask", "E_UNSUPPORTED", "not enabled"),
            "in failed type=ask E_UNSUPPORTED: not enabled"
        );
    }

    #[test]
    fn a_set_keeps_a_plain_value_but_never_free_text_with_debug_log_off() {
        let voice = map(json!({"type": "set", "key": "voice", "value": "af_sarah"}));
        assert!(input_line(&voice, false, false)
            .unwrap()
            .contains("af_sarah"));
        let folder = map(json!({"type": "set", "key": "earcons_dir", "value": "C:/Users/Secret"}));
        assert!(!input_line(&folder, false, false)
            .unwrap()
            .contains("Secret"));
        let on = map(json!({"type": "set", "key": "earcons_on", "value": true}));
        assert!(input_line(&on, false, false)
            .unwrap()
            .contains(r#""value":true"#));
    }

    #[test]
    fn secrets_are_masked_even_with_debug_log_on() {
        let m = map(
            json!({"type": "ask", "channel": "c", "text": "run with API_KEY=abc123", "token": "t0k"}),
        );
        let line = input_line(&m, false, true).unwrap();
        assert!(!line.contains("abc123") && !line.contains("t0k"), "{line}");
    }

    #[test]
    fn refusals_before_hello_are_logged_at_most_once_per_window() {
        let gate = Gate::new(10);
        assert!(gate.allow(1_000));
        assert!(!gate.allow(1_001));
        assert!(!gate.allow(1_009));
        assert!(gate.allow(1_010));
    }

    #[test]
    fn with_debug_log_off_no_text_reaches_the_log() {
        let ask = map(json!({
            "type": "ask", "channel": "c1", "kind": "permission", "t": 1.0,
            "text": "Secret command", "options": [{"label": "Secret label"}],
            "hint": "Secret hint", "notes": "Secret notes",
        }));
        let line = input_line(&ask, false, false).unwrap();
        assert!(!line.contains("Secret"), "{line}");
        assert!(line.contains(r#""kind":"permission""#) && line.contains(r#""options":1"#));
        let stream = map(
            json!({"type": "stream", "channel": "c1", "delta": "Secret prose", "index": 0, "final": true}),
        );
        assert!(!input_line(&stream, false, false)
            .unwrap()
            .contains("Secret"));
        let prefs = map(
            json!({"type": "set", "key": "channel_prefs", "value": {"c1": {"label": "Secret"}}}),
        );
        assert!(!input_line(&prefs, false, false).unwrap().contains("Secret"));
        let summaries = map(json!({
            "type": "set", "key": "summaries",
            "value": {"mode": "on", "prompts": {"turn": "Secret prompt"}},
        }));
        let line = input_line(&summaries, false, false).unwrap();
        assert!(!line.contains("Secret"), "{line}");
        assert!(line.contains(r#""key":"summaries""#), "{line}");
        let rate = map(json!({"type": "set", "key": "rate", "value": 275}));
        assert!(input_line(&rate, false, false)
            .unwrap()
            .contains(r#""value":275"#));
        let spoken = Trace {
            source: "stream".into(),
            channel: Some("c1".into()),
            what: Traced::Spoken {
                kind: "prose",
                entry: 3,
                text: "Secret prose".into(),
                decision: false,
                waits: None,
            },
        };
        assert_eq!(
            agent_line(&spoken, false),
            "agent stream channel=c1 speak kind=prose entry=3"
        );
        assert_eq!(
            agent_line(&spoken, true),
            "agent stream channel=c1 speak kind=prose entry=3 text=\"Secret prose\""
        );
        let d = Dropped {
            channel: "c1".into(),
            entry: 3,
            text: "Secret prose".into(),
            reason: "turn_start".into(),
            item: Some(sonara_reader::ItemId(9)),
        };
        let o = Origin {
            kind: "prose".into(),
            source: "stream".into(),
        };
        assert_eq!(
            drop_line(&d, Some(&o), false),
            "drop channel=c1 entry=3 item=9 (cut while read) kind=prose from=stream reason=turn_start"
        );
        assert!(drop_line(&d, None, true).ends_with("text=\"Secret prose\""));
    }

    #[test]
    fn agent_lines_say_why_nothing_was_spoken() {
        let note = Trace {
            source: "ask permission".into(),
            channel: Some("c1".into()),
            what: Traced::Note(Note {
                channel: Some("c1".into()),
                kind: "permission",
                what: "not spoken: the permission prompt of the question awaiting its answer (#11)"
                    .into(),
                text: Some("Claude needs your permission".into()),
            }),
        };
        assert_eq!(
            agent_line(&note, true),
            "agent \"ask permission\" channel=c1 permission: not spoken: the permission prompt \
             of the question awaiting its answer (#11) text=\"Claude needs your permission\""
        );
        let late = Trace {
            source: "stream".into(),
            channel: Some("c1".into()),
            what: Traced::Late,
        };
        assert!(agent_line(&late, true).contains("dropped: late text"));
        let e = Trace {
            source: "turn_end".into(),
            channel: Some("c1".into()),
            what: Traced::Earcon(Earcon::TurnDone),
        };
        assert_eq!(
            agent_line(&e, true),
            "agent turn_end channel=c1 earcon turn_done"
        );
    }

    #[test]
    fn huge_fields_are_clipped_and_origins_are_bounded() {
        let big = map(json!({"type": "speak", "text": "x".repeat(10_000)}));
        let line = input_line(&big, false, true).unwrap();
        assert!(
            line.len() < 5_000 && line.contains("...[+"),
            "{}",
            line.len()
        );
        let o = Origins::default();
        for i in 0..(ORIGINS as u64 + 10) {
            o.record(i, "prose", "stream");
        }
        assert!(o.get(0).is_none(), "the oldest went");
        assert_eq!(o.get(ORIGINS as u64 + 9).unwrap().kind, "prose");
    }
}
