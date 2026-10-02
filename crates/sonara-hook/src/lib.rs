//! Sonara L5: the Claude Code hook adapter. `sonara-hook.exe <Event>` gets
//! a Claude Code hook event (its name in argv, its JSON payload on stdin),
//! maps it to protocol v1 `channels` and `agent` messages (`map_event`,
//! pure, the port of the Python plugin's `hooks_entry.handle_event`), finds
//! the running `sonarad` through `runtime.json` and sends the messages as
//! one batch on one connection, so they apply in order. It never fails the
//! Claude session: every error is swallowed and the exit code is 0.
//!
//! The Claude product's runtime: `hello` asks for `agent` and `system`
//! (hotkeys, ducking, the settings page, spoken cues) with `keep_alive`,
//! so the runtime stays up and armed like the Python daemon did. When no
//! runtime answers, the hook **starts** `sonarad.exe` from its own folder
//! (detached, no window; the home comes from the same environment), waits
//! briefly for its `runtime.json` and sends then (`deliver`). The whole
//! start is bounded (`START_BUDGET`): a hook never holds up Claude Code for
//! long, and an event that misses the budget is dropped (the runtime keeps
//! starting for the next one). `SONARA_NO_START` (non-empty) turns the
//! start off; `SONARA_RUNTIME_ARGS` adds arguments to the runtime's
//! command line (a testing aid: `--engine fake --system fake`).
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
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a hook may spend starting the runtime and waiting for it.
pub const START_BUDGET: Duration = Duration::from_secs(1);
/// How long a connection attempt to a runtime named in `runtime.json` may
/// take (a live runtime on loopback answers at once; a dead one's port can
/// take seconds to refuse on Windows).
pub const PROBE: Duration = Duration::from_millis(300);
/// How often the hook looks for the new runtime's `runtime.json`.
const POLL: Duration = Duration::from_millis(25);
/// The runtime's file name next to `sonara-hook.exe`.
pub const RUNTIME_EXE: &str = "sonarad.exe";

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
    pub pid: Option<u64>,
}

pub fn read_runtime(home: &Path) -> Option<Runtime> {
    let raw = std::fs::read(home.join("runtime.json")).ok()?;
    let v: Value = serde_json::from_slice(&raw).ok()?;
    Some(Runtime {
        port: u16::try_from(v.get("port")?.as_u64()?).ok()?,
        token: v.get("token")?.as_str()?.to_string(),
        pid: v.get("pid").and_then(Value::as_u64),
    })
}

/// Connect to the runtime.
pub fn connect(rt: &Runtime, timeout: Duration) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], rt.port));
    TcpStream::connect_timeout(&addr, timeout)
}

/// The `hello` of the Claude product: `agent` and `system`, kept alive.
pub fn hello(token: &str) -> Value {
    json!({
        "type": "hello",
        "token": token,
        "client": {"name": "sonara-hook", "version": env!("CARGO_PKG_VERSION")},
        "extensions": ["agent", "system"],
        "keep_alive": true,
    })
}

/// Send `hello` and `msgs` on one connection, then read the replies so the
/// runtime has applied them all before the connection closes. Returns the
/// replies (hello first).
pub fn send(rt: &Runtime, msgs: &[Value], timeout: Duration) -> std::io::Result<Vec<Value>> {
    let s = connect(rt, timeout)?;
    send_on(s, rt, msgs, timeout)
}

/// `send` on a connection already made.
pub fn send_on(
    mut s: TcpStream,
    rt: &Runtime,
    msgs: &[Value],
    timeout: Duration,
) -> std::io::Result<Vec<Value>> {
    s.set_nodelay(true)?;
    let hello = hello(&rt.token);
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

/// `sonarad.exe` next to the running hook, if it is there.
pub fn runtime_exe(hook_exe: &Path) -> Option<PathBuf> {
    let exe = hook_exe.parent()?.join(RUNTIME_EXE);
    exe.is_file().then_some(exe)
}

/// The extra runtime arguments of `SONARA_RUNTIME_ARGS` (split on
/// whitespace).
pub fn runtime_args(env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    env("SONARA_RUNTIME_ARGS")
        .map(|a| a.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Keep the hook's standard handles out of the runtime it starts. Windows
/// gives a child every inheritable handle of its parent, and the hook's
/// stdin, stdout and stderr are Claude Code's pipes: a runtime holding
/// them would keep Claude Code waiting for the hook's output to end for as
/// long as the runtime lives.
#[cfg(windows)]
fn keep_std_handles_to_self() {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn SetHandleInformation(handle: *mut core::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    let handles = [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ];
    for h in handles {
        if !h.is_null() && h as isize != -1 {
            // SAFETY: a handle of this process; clearing its inherit flag
            // changes nothing else (a failure leaves it as it was).
            unsafe {
                SetHandleInformation(h.cast(), HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[cfg(not(windows))]
fn keep_std_handles_to_self() {}

/// Start the runtime detached, with no window and none of the hook's
/// handles, out of Claude Code's job when the job allows it (so it outlives
/// the hook).
pub fn start_runtime(exe: &Path, args: &[String]) -> std::io::Result<()> {
    keep_std_handles_to_self();
    let spawn = |flags: u32| {
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(flags);
        }
        #[cfg(not(windows))]
        let _ = flags;
        cmd.spawn().map(|_| ())
    };
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let base = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    spawn(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(base))
}

/// What `deliver` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Sent to the runtime that was running.
    Sent,
    /// Started the runtime, then sent.
    Started,
    /// Nothing reached a runtime (none running and none started in time,
    /// or the start is off).
    Dropped,
}

/// Send `msgs` to the runtime of `home`, starting `exe` (with `args`) when
/// none answers and `exe` is given; the start and the wait for it end by
/// `deadline`. Never blocks past the deadline for the start, nor past
/// `timeout` for the replies.
pub fn deliver(
    home: &Path,
    msgs: &[Value],
    exe: Option<&Path>,
    args: &[String],
    deadline: Instant,
    timeout: Duration,
) -> Delivery {
    let before = read_runtime(home);
    if let Some(rt) = &before {
        if let Ok(s) = connect(rt, PROBE) {
            let _ = send_on(s, rt, msgs, timeout);
            return Delivery::Sent;
        }
    }
    let Some(exe) = exe else {
        return Delivery::Dropped;
    };
    if start_runtime(exe, args).is_err() {
        return Delivery::Dropped;
    }
    let stale = before.and_then(|r| r.pid);
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        let Some(rt) = read_runtime(home).filter(|r| r.pid.is_none() || r.pid != stale) else {
            continue;
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if let Ok(s) = connect(&rt, left.min(PROBE)) {
            let _ = send_on(s, &rt, msgs, timeout);
            return Delivery::Started;
        }
    }
    Delivery::Dropped
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
                token: "abc".into(),
                pid: None,
            })
        );
        std::fs::write(dir.join("runtime.json"), "{not json").unwrap();
        assert_eq!(read_runtime(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_product_hello_asks_for_agent_and_system_kept_alive() {
        let h = hello("tok");
        assert_eq!(h["extensions"], json!(["agent", "system"]));
        assert_eq!(h["keep_alive"], true);
        assert_eq!(h["token"], "tok");
    }

    #[test]
    fn runtime_args_and_exe() {
        let env = |k: &str| (k == "SONARA_RUNTIME_ARGS").then(|| " --engine  fake ".to_string());
        assert_eq!(runtime_args(&env), ["--engine", "fake"]);
        assert!(runtime_args(&|_| None).is_empty());
        let dir = std::env::temp_dir().join(format!("sonara-hook-exe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hook = dir.join("sonara-hook.exe");
        assert_eq!(runtime_exe(&hook), None, "no runtime next to it");
        std::fs::write(dir.join(RUNTIME_EXE), b"").unwrap();
        assert_eq!(runtime_exe(&hook), Some(dir.join(RUNTIME_EXE)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_runtime_or_an_exe_nothing_is_delivered_at_once() {
        let dir = std::env::temp_dir().join(format!("sonara-hook-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = Instant::now();
        let d = deliver(
            &dir,
            &[json!({"type": "stream"})],
            None,
            &[],
            t + START_BUDGET,
            Duration::from_secs(2),
        );
        assert_eq!(d, Delivery::Dropped);
        assert!(t.elapsed() < Duration::from_millis(500));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stamping_adds_t_to_every_message() {
        let mut msgs = vec![json!({"type": "stream"}), json!({"type": "focus"})];
        stamp(&mut msgs, 12.5);
        assert!(msgs.iter().all(|m| m["t"] == 12.5));
    }
}
