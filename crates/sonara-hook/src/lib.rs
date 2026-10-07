//! Sonara L5: the Claude Code hook adapter. `sonara-hook.exe <Event>` gets
//! a Claude Code hook event (its name in argv, its JSON payload on stdin),
//! maps it to protocol v1 `channels` and `agent` messages (`map_event`,
//! pure, the port of the Python plugin's `hooks_entry.handle_event`) and
//! hands them to the protocol client (`sonara_client::deliver`, #255),
//! which finds the running `sonarad` through `runtime.json` and sends them
//! as one batch on one connection, so they apply in order. This crate
//! keeps only the mapping, the hook's `hello` and its log. It never fails
//! the Claude session: every error is swallowed and the exit code is 0.
//!
//! The Claude product's runtime: `hello` (`HELLO`) asks for `agent` and
//! `system` (hotkeys, ducking, the settings page, spoken cues) with
//! `keep_alive`, so the runtime stays up and armed like the Python daemon
//! did. When no runtime answers, the hook **starts** `sonarad.exe` from its
//! own folder (detached, no window; the home comes from the same
//! environment), waits briefly for its `runtime.json` and sends then. The
//! whole start is bounded (`START_BUDGET`): a hook never holds up Claude
//! Code for long, and an event that misses the budget is dropped (the
//! runtime keeps starting for the next one). `SONARA_NO_START` (non-empty)
//! turns the start off, and so does the stop sentinel `<home>/stopped`
//! (`sonara_client::STOPPED`); `SONARA_RUNTIME_ARGS` adds arguments to the
//! runtime's command line (the plugin's `bin/sonara-hook-launch` passes
//! `--standalone`; tests `--engine fake --system fake`).
//!
//! One Claude session is one channel (its `session_id`). Each message is
//! stamped with `t`, the hook process's start time, so text of a turn that
//! arrives after the next prompt is dropped by the runtime (#174). The
//! session's project (`project_label`, #245: the repository `cwd` is in,
//! a worktree's main repository, else `cwd`'s folder) is the channel's
//! label: on `channel_open` (with `keep_label`, so the runtime keeps the
//! first one a session got), and as `label` on every `agent` message that names the
//! session (`stream`, `tool`, `ask`, `answered`, `turn_end`; `LABELLED`),
//! so a session whose first message after a runtime restart is not its
//! prompt is still named when Sonara switches to it (#241).
//!
//! Mapping (Python message names in brackets):
//! - `MessageDisplay` -> `stream` (PROSE).
//! - `PreToolUse` `AskUserQuestion` -> one `ask` `question` per question
//!   (EARCON choice + CHOICE); the Claude TUI's key notes and selection
//!   hints ride on the first one. Of two or more, each carries `set_index`
//!   and `set_size` (#283): the runtime reads only the first and the
//!   question hotkeys move between them. When the question's message has
//!   no text block, its last thinking block (read from the end of the
//!   session's transcript, `transcript`) goes first as one final `stream`,
//!   so the answer before the question is read (#283).
//!   `ExitPlanMode` -> `ask` `plan` (PLAN).
//!   Any other tool -> `tool` with a short summary (TOOL).
//! - `PostToolUse` `AskUserQuestion` -> `answered` (CHOICE_ANSWERED).
//! - `Notification` `permission_prompt` -> `ask` `permission` (EARCON
//!   permission + PERMISSION); other notifications send nothing.
//! - `Stop` -> `turn_end` (EARCON turn_done).
//! - `UserPromptSubmit` -> `channel_open`, `focus`, `turn_start`
//!   (SET_FOREGROUND + FLUSH).
//! - `SessionStart` -> `channel_open`, `focus` (SET_FOREGROUND +
//!   SESSION_START). `SessionEnd` -> `channel_close` (SESSION_END).
//!
//! **Troubleshooting log** (#219): every invocation appends one line to
//! `<home>\logs\hook.log` (`sonara_log`: the folder's 10 MB budget, oldest
//! first out): the event, the outcome (`sent`, `started`, `dropped`,
//! `nothing to send`), the time it took, the messages sent and the raw
//! payload (a string field over `FIELD_MAX` clipped). With `debug_log`
//! off in `config.json` the payload and the messages' text are left out.
//! The log never holds up or fails the hook: the lock is waited for
//! `LOG_WAIT` at most and every error is ignored.
use serde_json::{json, Map, Value};
use sonara_client::{Delivery, Hello, PRODUCT};
/// The log's masking and clipping, shared with the other writer.
pub use sonara_log::{scrub, FIELD_MAX};
use std::path::Path;
use std::time::Duration;

mod project;
pub mod transcript;
pub use project::{basename, project_label};

/// How long a hook may spend starting the runtime and waiting for it.
pub const START_BUDGET: Duration = Duration::from_secs(1);

/// The `hello` of the Claude product: `agent` and `system`, kept alive.
pub const HELLO: Hello<'static> = Hello {
    name: "sonara-hook",
    version: env!("CARGO_PKG_VERSION"),
    extensions: PRODUCT,
    keep_alive: true,
};

/// Spoken after a decision at verbosity `everything`: how to answer in the
/// Claude Code TUI.
pub const SELECT_HINT: &str = "Press the option's number to choose, or Escape to cancel.";
/// Spoken once per session after the hint.
pub const SELECT_ONCE: &str = "Selecting is immediate.";
const MULTI_NOTE: &str =
    "Select multiple: press each number, or Space on the highlighted item, then Enter to confirm.";
const MANY_NOTE: &str = "More than nine options; use arrow keys for ten and up.";

/// The hook's stream in the log folder (`hook.log`).
pub const LOG_STREAM: &str = "hook";
/// How long a hook waits for another writer of the log before it skips
/// its line.
pub const LOG_WAIT: Duration = Duration::from_millis(50);

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

/// The session's label: the project it works in (`project_label`, #245;
/// the walk to `.git` ends at the user's home, `USERPROFILE`).
fn label(payload: &Value, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    project_label(text(payload, "cwd"), env("USERPROFILE").as_deref())
}

/// The `agent` messages that carry the session's label (#241).
/// `turn_start` comes right after a `channel_open` with it.
pub const LABELLED: &[&str] = &["stream", "tool", "ask", "answered", "turn_end"];

/// Add the session's label to each message in `LABELLED`.
fn with_label(mut msgs: Vec<Value>, label: Option<&str>) -> Vec<Value> {
    if let Some(l) = label {
        for m in &mut msgs {
            if let Value::Object(o) = m {
                if o.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| LABELLED.contains(&t))
                {
                    o.insert("label".into(), json!(l));
                }
            }
        }
    }
    msgs
}

/// `channel_open` with the session's project as its label, kept when the
/// channel already has one (`keep_label`, #245: a session keeps its name).
fn open(channel: &str, label: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Value {
    let mut m = msg("channel_open", channel);
    if let Some(project) = label {
        m.insert("label".into(), json!(project));
        m.insert("keep_label".into(), json!(true));
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
    // The hints ride on the first question, the one read (#283).
    let first = out.remove(0);
    let mut first = with_hints(first);
    if !notes.is_empty() {
        first.insert("notes".into(), json!(notes.join(" ")));
    }
    out.insert(0, first);
    let size = out.len();
    if size >= 2 {
        for (i, m) in out.iter_mut().enumerate() {
            m.insert("set_index".into(), json!(i));
            m.insert("set_size".into(), json!(size));
        }
    }
    out.into_iter().map(Value::Object).collect()
}

/// How the hook finds the lead-in of a question (#283): the transcript's
/// path and the tool use's id give the message's last thinking block when
/// it has no text (`transcript::lead_in`).
pub type LeadIn<'a> = &'a dyn Fn(&Path, &str) -> Option<String>;

/// The lead-in before the questions of `payload`, as one final `stream`
/// (the prose rules apply: held and released by the question, skipped in a
/// flushed reply). A subagent's question (`agent_id`) has its tool use in
/// another transcript: no lead-in.
fn thinking_lead_in(channel: &str, payload: &Value, lead_in: LeadIn) -> Option<Value> {
    if !text(payload, "agent_id").is_empty() {
        return None;
    }
    let path = text(payload, "transcript_path");
    let id = text(payload, "tool_use_id");
    if path.is_empty() || id.is_empty() {
        return None;
    }
    let t = lead_in(Path::new(path), id)?;
    let mut m = msg("stream", channel);
    m.insert("delta".into(), json!(t));
    m.insert("index".into(), json!(0));
    m.insert("final".into(), json!(true));
    Some(Value::Object(m))
}

/// Map one hook event to protocol messages (no `t` yet). Pure: the
/// environment is read through `env`. Unknown events map to nothing.
/// The label is looked up only for an event that sends something: the walk
/// to `.git` touches the file system, and most tool events send nothing.
pub fn map_event(event: &str, payload: &Value, env: &dyn Fn(&str) -> Option<String>) -> Vec<Value> {
    map_event_with(event, payload, env, &|_, _| None)
}

/// `map_event`, with `lead_in` finding the thinking to read before a
/// question whose message has no text (#283; `main` passes
/// `transcript::lead_in`, the tests a pure stand-in).
pub fn map_event_with(
    event: &str,
    payload: &Value,
    env: &dyn Fn(&str) -> Option<String>,
    lead_in: LeadIn,
) -> Vec<Value> {
    let cell = std::cell::OnceCell::new();
    let lazy = || cell.get_or_init(|| label(payload, env)).clone();
    let msgs = map_unlabelled(event, payload, &lazy, env, lead_in);
    if msgs.is_empty() {
        return msgs;
    }
    with_label(msgs, lazy().as_deref())
}

fn map_unlabelled(
    event: &str,
    payload: &Value,
    label: &dyn Fn() -> Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
    lead_in: LeadIn,
) -> Vec<Value> {
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
                "AskUserQuestion" => {
                    let mut out: Vec<Value> = thinking_lead_in(channel, payload, lead_in)
                        .into_iter()
                        .collect();
                    out.extend(questions(channel, input));
                    out
                }
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
            open(channel, label().as_deref(), env),
            Value::Object(msg("focus", channel)),
            Value::Object(msg("turn_start", channel)),
        ],
        "SessionStart" => vec![
            open(channel, label().as_deref(), env),
            Value::Object(msg("focus", channel)),
        ],
        "SessionEnd" => vec![Value::Object(msg("channel_close", channel))],
        _ => Vec::new(),
    }
}

/// Whether text and payloads go to the log: `debug_log` in the home's
/// `config.json`, true unless set to false.
pub fn debug_log(home: &Path) -> bool {
    std::fs::read(home.join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|v| v.get("debug_log").and_then(Value::as_bool))
        .unwrap_or(true)
}

/// Tools whose input Sonara reads, so the log keeps it whole.
const READ_TOOLS: &[&str] = &["AskUserQuestion"];

/// Every tool fires `PreToolUse`, so its `tool_input` may be a shell
/// command or a whole file: for a tool Sonara does not read, only the
/// input's field names are kept.
pub fn slim_tool_input(payload: &mut Value) {
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    if READ_TOOLS.contains(&tool) {
        return;
    }
    if let Some(input) = payload.get_mut("tool_input") {
        if let Value::Object(o) = input {
            *input = json!({ "fields": o.keys().collect::<Vec<_>>() });
        } else if !input.is_null() {
            *input = json!("[omitted]");
        }
    }
    if let Some(out) = payload.get_mut("tool_response") {
        *out = json!("[omitted]");
    }
}

/// The log line of one invocation (module docs). `raw` is stdin as read;
/// `outcome` what became of the messages.
pub fn log_line(
    event: &str,
    raw: &[u8],
    msgs: &[Value],
    outcome: &str,
    elapsed: Duration,
    debug: bool,
) -> String {
    let payload: Option<Value> = serde_json::from_slice(raw).ok();
    let mut line = format!(
        "hook {} pid={} {outcome} ms={}",
        if event.is_empty() { "?" } else { event },
        std::process::id(),
        elapsed.as_millis()
    );
    let field = |key: &str| {
        payload
            .as_ref()
            .and_then(|p| p.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    if let Some(sid) = field("session_id") {
        line.push_str(&format!(" session={sid}"));
    }
    for (key, name) in [("tool_name", "tool"), ("notification_type", "notification")] {
        if let Some(v) = field(key) {
            line.push_str(&format!(" {name}={}", Value::String(v)));
        }
    }
    if debug {
        let mut sent = Value::Array(msgs.to_vec());
        scrub(&mut sent);
        line.push_str(&format!(" sent={sent}"));
        match payload {
            Some(mut p) => {
                slim_tool_input(&mut p);
                scrub(&mut p);
                line.push_str(&format!(" payload={p}"));
            }
            None if raw.is_empty() => line.push_str(" payload=none"),
            None => {
                let text = String::from_utf8_lossy(raw);
                let text = sonara_log::clip(&text, FIELD_MAX).into_owned();
                line.push_str(&format!(" raw={}", Value::String(text)));
            }
        }
    } else {
        let types: Vec<&str> = msgs
            .iter()
            .filter_map(|m| m.get("type").and_then(Value::as_str))
            .collect();
        line.push_str(&format!(" sent={}", json!(types)));
    }
    line
}

/// Append `line` to the home's `hook.log`, best effort: a busy log (another
/// writer past `LOG_WAIT`) or any error skips it.
pub fn log(home: &Path, line: &str) {
    let _ = sonara_log::LogDir::new(home.join("logs"))
        .with_wait(LOG_WAIT)
        .log(LOG_STREAM, line);
}

/// What became of the messages, for the log.
pub fn outcome(d: Option<Delivery>) -> &'static str {
    match d {
        None => "nothing to send",
        Some(Delivery::Sent) => "sent",
        Some(Delivery::Started) => "started the runtime, sent",
        Some(Delivery::Dropped) => "dropped (no runtime answered)",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_that_sends_nothing_never_looks_for_the_project() {
        // The walk to `.git` reads USERPROFILE first: an ignored event
        // (most PostToolUse and Notification events) must not walk at all.
        let asked = std::cell::Cell::new(false);
        let env = |k: &str| {
            if k == "USERPROFILE" {
                asked.set(true);
            }
            None
        };
        let p = json!({"session_id": "s", "cwd": r"C:\x\proj", "tool_name": "Bash"});
        assert!(map_event("PostToolUse", &p, &env).is_empty());
        let n = json!({"session_id": "s", "cwd": r"C:\x\proj", "notification_type": "idle"});
        assert!(map_event("Notification", &n, &env).is_empty());
        assert!(!asked.get());
        // An event that sends something still gets the label.
        let out = map_event("Stop", &p, &env);
        assert_eq!(out[0]["label"], json!("proj"));
        assert!(asked.get());
    }

    fn ask_payload(n: usize) -> Value {
        let qs: Vec<Value> = (1..=n)
            .map(|i| json!({"question": format!("Q{i}?"), "options": [{"label": "A"}]}))
            .collect();
        json!({"session_id": "s", "tool_name": "AskUserQuestion",
               "transcript_path": r"C:\t\s.jsonl", "tool_use_id": "toolu_1",
               "tool_input": {"questions": qs}})
    }

    #[test]
    fn the_thinking_goes_as_one_final_stream_before_the_asks() {
        let asked = std::cell::RefCell::new(Vec::new());
        let lead = |p: &Path, id: &str| {
            asked
                .borrow_mut()
                .push((p.display().to_string(), id.to_string()));
            Some("The answer before the question.".to_string())
        };
        let out = map_event_with("PreToolUse", &ask_payload(2), &|_| None, &lead);
        assert_eq!(
            asked.borrow().as_slice(),
            [(r"C:\t\s.jsonl".to_string(), "toolu_1".to_string())]
        );
        assert_eq!(out.len(), 3, "{out:?}");
        assert_eq!(
            out[0],
            json!({"type": "stream", "channel": "s", "index": 0, "final": true,
                   "delta": "The answer before the question."})
        );
        assert_eq!(out[1]["type"], "ask");
        // No lead-in (a text block, or nothing found): the asks alone.
        let out = map_event_with("PreToolUse", &ask_payload(2), &|_| None, &|_, _| None);
        assert!(out.iter().all(|m| m["type"] == "ask"));
        // A subagent's question is not looked up.
        let mut sub = ask_payload(1);
        sub["agent_id"] = json!("agent-1");
        let out = map_event_with("PreToolUse", &sub, &|_| None, &|_, _| panic!("looked up"));
        assert_eq!(out.len(), 1);
        // Other tools never read the transcript.
        let bash = json!({"session_id": "s", "tool_name": "Bash", "transcript_path": "x",
                          "tool_use_id": "t"});
        map_event_with("PreToolUse", &bash, &|_| None, &|_, _| panic!("looked up"));
    }

    #[test]
    fn a_question_set_carries_set_index_and_size_and_its_hints_on_the_first() {
        let out = map_event("PreToolUse", &ask_payload(3), &|_| None);
        assert_eq!(out.len(), 3);
        for (i, m) in out.iter().enumerate() {
            assert_eq!(
                (m["set_index"].clone(), m["set_size"].clone()),
                (json!(i), json!(3))
            );
            assert_eq!(m.get("hint").is_some(), i == 0, "{m}");
        }
        let one = map_event("PreToolUse", &ask_payload(1), &|_| None);
        assert!(one[0].get("set_index").is_none() && one[0].get("hint").is_some());
    }

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
    fn the_product_hello_asks_for_agent_and_system_kept_alive() {
        let h = HELLO.message("tok");
        assert_eq!(h["extensions"], json!(["agent", "system"]));
        assert_eq!(h["keep_alive"], true);
        assert_eq!(h["token"], "tok");
    }

    #[test]
    fn stamping_adds_t_to_every_message() {
        let mut msgs = vec![json!({"type": "stream"}), json!({"type": "focus"})];
        stamp(&mut msgs, 12.5);
        assert!(msgs.iter().all(|m| m["t"] == 12.5));
    }

    #[test]
    fn the_log_keeps_no_command_file_or_secret_of_a_tool_sonara_does_not_read() {
        let raw = json!({
            "hook_event_name": "PreToolUse",
            "session_id": "s1",
            "tool_name": "Bash",
            "tool_input": {"command": "curl -H 'Authorization: Bearer abcdefghijklmnop' x"},
            "api_key": "plain-secret",
        })
        .to_string();
        let line = log_line(
            "PreToolUse",
            raw.as_bytes(),
            &[],
            "sent",
            Duration::ZERO,
            true,
        );
        assert!(
            line.contains(r#""tool_input":{"fields":["command"]}"#),
            "{line}"
        );
        assert!(
            !line.contains("abcdefghijklmnop") && !line.contains("plain-secret"),
            "{line}"
        );
        assert!(line.contains(r#""api_key":"[redacted]""#), "{line}");
    }

    #[test]
    fn the_log_keeps_the_question_sonara_reads_with_secrets_masked() {
        let raw = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": {"questions": [{"question": "Use GITHUB_TOKEN=ghp_1234567890abcdef1234 ?",
                                          "options": [{"label": "Red"}]}]},
        })
        .to_string();
        let line = log_line(
            "PreToolUse",
            raw.as_bytes(),
            &[],
            "sent",
            Duration::ZERO,
            true,
        );
        assert!(
            line.contains("Use GITHUB_TOKEN=[redacted] ?") && line.contains("Red"),
            "{line}"
        );
        assert!(!line.contains("ghp_1234567890"), "{line}");
    }

    #[test]
    fn with_debug_log_off_the_log_has_no_payload_and_no_text() {
        let raw = json!({"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Secret?"}]}})
            .to_string();
        let msgs = vec![json!({"type": "ask", "text": "Secret?"})];
        let line = log_line(
            "PreToolUse",
            raw.as_bytes(),
            &msgs,
            "sent",
            Duration::ZERO,
            false,
        );
        assert!(
            !line.contains("Secret") && !line.contains("payload"),
            "{line}"
        );
    }
}
