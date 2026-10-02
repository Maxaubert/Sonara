//! Sonara L5: the Claude Code hook adapter. `sonara-hook.exe <Event>` gets
//! a Claude Code hook event (its name in argv, its JSON payload on stdin),
//! maps it to protocol v1 `channels` and `agent` messages (`map_event`,
//! pure, the port of the Python plugin's `hooks_entry.handle_event`), finds
//! the running `sonarad` through `runtime.json` and sends the messages as
//! one batch on one connection, so they apply in order. It never fails the
//! Claude session: every error is swallowed and the exit code is 0.
//!
//! One Claude session is one channel (its `session_id`). Each message is
//! stamped with `t`, the hook process's start time, so text of a turn that
//! arrives after the next prompt is dropped by the runtime (#174).
//!
//! Mapping (Python message names in brackets):
//! - `MessageDisplay` -> `stream` (PROSE).
//! - `PreToolUse` `AskUserQuestion` -> one `ask` `question` per question
//!   (EARCON choice + CHOICE); the Claude TUI's key notes and selection
//!   hints ride on the last one. `ExitPlanMode` -> `ask` `plan` (PLAN).
//!   Any other tool -> `tool` with a short summary (TOOL).
//! - `PostToolUse` `AskUserQuestion` -> `answered` (CHOICE_ANSWERED).
//! - `Notification` `permission_prompt` -> `ask` `permission` (EARCON
//!   permission + PERMISSION); other notifications send nothing.
//! - `Stop` -> `turn_end` (EARCON turn_done).
//! - `UserPromptSubmit` -> `channel_open`, `focus`, `turn_start`
//!   (SET_FOREGROUND + FLUSH).
//! - `SessionStart` -> `channel_open`, `focus` (SET_FOREGROUND +
//!   SESSION_START). `SessionEnd` -> `channel_close` (SESSION_END).
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Spoken after a decision at verbosity `everything`: how to answer in the
/// Claude Code TUI.
pub const SELECT_HINT: &str = "Press the option's number to choose, or Escape to cancel.";
/// Spoken once per session after the hint.
pub const SELECT_ONCE: &str = "Selecting is immediate.";
const MULTI_NOTE: &str =
    "Select multiple: press each number, or Space on the highlighted item, then Enter to confirm.";
const MANY_NOTE: &str = "More than nine options; use arrow keys for ten and up.";

/// A session without an id (never seen from Claude Code) still gets a
/// channel.
pub const DEFAULT_CHANNEL: &str = "default";

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn msg(kind: &str, channel: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".into(), json!(kind));
    m.insert("channel".into(), json!(channel));
    m
}

/// The last path component (either separator), as `os.path.basename` on
/// Windows.
fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit(['/', '\\']).next().unwrap_or("")
}

/// A short, speakable description of a pending tool call.
pub fn tool_summary(tool: &str, input: &Value) -> String {
    match tool {
        "Bash" => {
            let cmd = text(input, "command").trim();
            if cmd.is_empty() {
                "Bash".into()
            } else {
                cmd.chars().take(120).collect()
            }
        }
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            let path = match text(input, "file_path") {
                "" => text(input, "notebook_path"),
                p => p,
            };
            match basename(path) {
                "" => tool.to_string(),
                b => b.to_string(),
            }
        }
        _ => tool.to_string(),
    }
}

/// The embedding host's tab (`SONARA_HOST_TAB`, or PrismTerminal's
/// `PRISM_TAB_ID`), if the hook runs inside one.
fn host_tab(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    ["SONARA_HOST_TAB", "PRISM_TAB_ID"]
        .iter()
        .filter_map(|k| env(k))
        .find(|v| !v.is_empty())
}

/// `channel_open` with the session's folder as its label.
fn open(channel: &str, payload: &Value, env: &dyn Fn(&str) -> Option<String>) -> Value {
    let mut m = msg("channel_open", channel);
    let folder = basename(text(payload, "cwd"));
    if !folder.is_empty() {
        m.insert("label".into(), json!(folder));
    }
    if let Some(tab) = host_tab(env) {
        m.insert("host_tab".into(), json!(tab));
    }
    Value::Object(m)
}

fn with_hints(mut m: Map<String, Value>) -> Map<String, Value> {
    m.insert("hint".into(), json!(SELECT_HINT));
    m.insert("hint_once".into(), json!(SELECT_ONCE));
    m
}

fn questions(channel: &str, input: &Value) -> Vec<Value> {
    let qs: Vec<Value> = input
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let multi = |q: &Value| q.get("multiSelect").and_then(Value::as_bool) == Some(true);
    let options = |q: &Value| q.get("options").and_then(Value::as_array).cloned();
    let mut notes = Vec::new();
    if qs.iter().any(multi) {
        notes.push(MULTI_NOTE);
    }
    if qs.iter().any(|q| options(q).is_some_and(|o| o.len() > 9)) {
        notes.push(MANY_NOTE);
    }
    let mut out: Vec<Map<String, Value>> = qs
        .iter()
        .map(|q| {
            let mut m = msg("ask", channel);
            m.insert("kind".into(), json!("question"));
            let (text_value, opts) = match q {
                Value::Object(_) => (
                    text(q, "question").to_string(),
                    options(q).unwrap_or_default(),
                ),
                Value::String(s) => (s.clone(), Vec::new()),
                other => (other.to_string(), Vec::new()),
            };
            m.insert("text".into(), json!(text_value));
            let opts: Vec<Value> = opts
                .iter()
                .map(|o| match o {
                    Value::Object(_) => {
                        let mut c = Map::new();
                        c.insert("label".into(), json!(text(o, "label")));
                        let d = text(o, "description");
                        if !d.trim().is_empty() {
                            c.insert("description".into(), json!(d));
                        }
                        Value::Object(c)
                    }
                    Value::String(s) => json!({"label": s}),
                    other => json!({"label": other.to_string()}),
                })
                .collect();
            m.insert("options".into(), Value::Array(opts));
            if multi(q) {
                m.insert("multi_select".into(), json!(true));
            }
            m
        })
        .collect();
    if out.is_empty() {
        let mut m = msg("ask", channel);
        m.insert("kind".into(), json!("question"));
        m.insert("text".into(), json!(""));
        out.push(m);
    }
    let last = out.pop().expect("one at least");
    let mut last = with_hints(last);
    if !notes.is_empty() {
        last.insert("notes".into(), json!(notes.join(" ")));
    }
    out.push(last);
    out.into_iter().map(Value::Object).collect()
}

/// Map one hook event to protocol messages (no `t` yet). Pure: the
/// environment is read through `env`. Unknown events map to nothing.
pub fn map_event(event: &str, payload: &Value, env: &dyn Fn(&str) -> Option<String>) -> Vec<Value> {
    let channel = match text(payload, "session_id") {
        "" => DEFAULT_CHANNEL,
        s => s,
    };
    match event {
        "MessageDisplay" => {
            let mut m = msg("stream", channel);
            m.insert("delta".into(), json!(text(payload, "delta")));
            let index = payload.get("index").and_then(Value::as_u64).unwrap_or(0);
            m.insert("index".into(), json!(index));
            let fin = payload
                .get("final")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            m.insert("final".into(), json!(fin));
            vec![Value::Object(m)]
        }
        "PreToolUse" => {
            let tool = text(payload, "tool_name");
            let empty = Value::Object(Map::new());
            let input = payload.get("tool_input").unwrap_or(&empty);
            match tool {
                "AskUserQuestion" => questions(channel, input),
                "ExitPlanMode" => {
                    let mut m = msg("ask", channel);
                    m.insert("kind".into(), json!("plan"));
                    m.insert("text".into(), json!(text(input, "plan")));
                    vec![Value::Object(with_hints(m))]
                }
                _ => {
                    let mut m = msg("tool", channel);
                    m.insert("name".into(), json!(tool));
                    m.insert("summary".into(), json!(tool_summary(tool, input)));
                    vec![Value::Object(m)]
                }
            }
        }
        "PostToolUse" if text(payload, "tool_name") == "AskUserQuestion" => {
            vec![Value::Object(msg("answered", channel))]
        }
        "Notification" => {
            let kind = match text(payload, "notification_type") {
                "" => text(payload, "matcher"),
                k => k,
            };
            if kind != "permission_prompt" {
                return Vec::new();
            }
            let action = text(payload, "action").trim();
            let what = if action.is_empty() {
                text(payload, "message").trim()
            } else {
                action
            };
            let mut m = msg("ask", channel);
            m.insert("kind".into(), json!("permission"));
            m.insert("text".into(), json!(what));
            vec![Value::Object(with_hints(m))]
        }
        "Stop" => vec![Value::Object(msg("turn_end", channel))],
        "UserPromptSubmit" => vec![
            open(channel, payload, env),
            Value::Object(msg("focus", channel)),
            Value::Object(msg("turn_start", channel)),
        ],
        "SessionStart" => vec![
            open(channel, payload, env),
            Value::Object(msg("focus", channel)),
        ],
        "SessionEnd" => vec![Value::Object(msg("channel_close", channel))],
        _ => Vec::new(),
    }
}

/// Stamp every message with the hook's start time.
pub fn stamp(msgs: &mut [Value], t: f64) {
    for m in msgs {
        if let Value::Object(o) = m {
            o.insert("t".into(), json!(t));
        }
    }
}

/// The runtime's home: `SONARA_HOME`, else `%LOCALAPPDATA%\Sonara` (as
/// `sonarad`).
pub fn home(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(h) = env("SONARA_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h));
    }
    env("LOCALAPPDATA")
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(&d).join("Sonara"))
}

/// Where and how to reach the runtime (from `runtime.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub port: u16,
    pub token: String,
}

pub fn read_runtime(home: &Path) -> Option<Runtime> {
    let raw = std::fs::read(home.join("runtime.json")).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    Some(Runtime {
        port: u16::try_from(v.get("port")?.as_u64()?).ok()?,
        token: v.get("token")?.as_str()?.to_string(),
    })
}

/// Send `hello` (enabling `agent`) and `msgs` on one connection, then read
/// the replies so the runtime has applied them all before the connection
/// closes. Returns the replies (hello first).
pub fn send(rt: &Runtime, msgs: &[Value], timeout: Duration) -> std::io::Result<Vec<Value>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], rt.port));
    let mut s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_nodelay(true)?;
    let hello = json!({
        "type": "hello",
        "token": rt.token,
        "client": {"name": "sonara-hook", "version": env!("CARGO_PKG_VERSION")},
        "extensions": ["agent"],
    });
    let mut batch = Vec::new();
    for m in std::iter::once(&hello).chain(msgs) {
        serde_json::to_writer(&mut batch, m)?;
        batch.push(b'\n');
    }
    s.write_all(&batch)?;
    s.flush()?;
    let end = Instant::now() + timeout;
    let mut replies = Vec::new();
    let mut r = BufReader::new(s);
    while replies.len() < msgs.len() + 1 {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        r.get_ref().set_read_timeout(Some(left))?;
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            break;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if v.get("ok").is_some() {
                replies.push(v);
            }
        }
    }
    Ok(replies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_summaries() {
        let bash = json!({"command": "  git status  "});
        assert_eq!(tool_summary("Bash", &bash), "git status");
        assert_eq!(tool_summary("Bash", &json!({})), "Bash");
        let long = json!({"command": "é".repeat(200)});
        assert_eq!(tool_summary("Bash", &long).chars().count(), 120);
        let w = json!({"file_path": "/Users/me/proj/src/sonara/cli.py"});
        assert_eq!(tool_summary("Write", &w), "cli.py");
        let e = json!({"file_path": r"C:\proj\README.md"});
        assert_eq!(tool_summary("Edit", &e), "README.md");
        let nb = json!({"notebook_path": "/a/b/n.ipynb"});
        assert_eq!(tool_summary("NotebookEdit", &nb), "n.ipynb");
        assert_eq!(tool_summary("Write", &json!({})), "Write");
        assert_eq!(tool_summary("WebFetch", &json!({})), "WebFetch");
    }

    #[test]
    fn home_and_runtime_file() {
        let env = |k: &str| match k {
            "LOCALAPPDATA" => Some(r"C:\Users\x\AppData\Local".to_string()),
            _ => None,
        };
        assert_eq!(
            home(&env),
            Some(PathBuf::from(r"C:\Users\x\AppData\Local\Sonara"))
        );
        let env2 = |k: &str| match k {
            "SONARA_HOME" => Some(r"D:\h".to_string()),
            "LOCALAPPDATA" => Some(r"C:\l".to_string()),
            _ => None,
        };
        assert_eq!(home(&env2), Some(PathBuf::from(r"D:\h")));
        assert_eq!(home(&|_| None), None);
        let dir = std::env::temp_dir().join(format!("sonara-hook-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_runtime(&dir), None);
        std::fs::write(
            dir.join("runtime.json"),
            r#"{"port": 5000, "token": "abc"}"#,
        )
        .unwrap();
        assert_eq!(
            read_runtime(&dir),
            Some(Runtime {
                port: 5000,
                token: "abc".into()
            })
        );
        std::fs::write(dir.join("runtime.json"), "{not json").unwrap();
        assert_eq!(read_runtime(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stamping_adds_t_to_every_message() {
        let mut msgs = vec![json!({"type": "stream"}), json!({"type": "focus"})];
        stamp(&mut msgs, 12.5);
        assert!(msgs.iter().all(|m| m["t"] == 12.5));
    }
}
