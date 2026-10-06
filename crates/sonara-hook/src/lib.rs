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
//! start off, and so does the stop sentinel `<home>/stopped` (`STOPPED`); `SONARA_RUNTIME_ARGS` adds arguments to the runtime's
//! command line (the plugin's `bin/sonara-hook-launch` passes `--standalone`;
//! tests `--engine fake --system fake`).
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
/// The log's masking and clipping, shared with the other writer.
pub use sonara_log::{scrub, FIELD_MAX};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

mod project;
pub use project::{basename, project_label};

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
/// The stop sentinel in the home: while it exists the hook never starts
/// the runtime (the user shut Sonara down), like the Python plugin's
/// `stopped` file. Events still reach a runtime that is running.
pub const STOPPED: &str = "stopped";

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
/// The label is looked up only for an event that sends something: the walk
/// to `.git` touches the file system, and most tool events send nothing.
pub fn map_event(event: &str, payload: &Value, env: &dyn Fn(&str) -> Option<String>) -> Vec<Value> {
    let cell = std::cell::OnceCell::new();
    let lazy = || cell.get_or_init(|| label(payload, env)).clone();
    let msgs = map_unlabelled(event, payload, &lazy, env);
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
pub fn start_runtime(exe: &Path, args: &[String]) -> std::io::Result<Child> {
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
        cmd.spawn()
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

/// Whether the user shut Sonara down in `home` (`STOPPED`).
pub fn stopped(home: &Path) -> bool {
    home.join(STOPPED).exists()
}

/// Whether the wait after a start should try `rt` (the `runtime.json` now
/// in the home): a runtime other than the `stale` one that did not answer,
/// or the stale one again once the start has exited (a second runtime
/// exits at once on the single-instance mutex, so the one named is alive,
/// only slow to answer the first probe).
pub fn worth_trying(rt: &Runtime, stale: Option<u64>, start_exited: bool) -> bool {
    start_exited || rt.pid.is_none() || rt.pid != stale
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
    let Some(exe) = exe.filter(|_| !stopped(home)) else {
        return Delivery::Dropped;
    };
    let Ok(mut child) = start_runtime(exe, args) else {
        return Delivery::Dropped;
    };
    let stale = before.and_then(|r| r.pid);
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        let exited = matches!(child.try_wait(), Ok(Some(_)));
        let Some(rt) = read_runtime(home).filter(|r| worth_trying(r, stale, exited)) else {
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
    fn the_wait_takes_a_new_runtime_or_the_old_one_once_the_start_is_gone() {
        let rt = |pid| Runtime {
            port: 1,
            token: "t".into(),
            pid,
        };
        assert!(worth_trying(&rt(Some(2)), Some(1), false), "a new runtime");
        assert!(worth_trying(&rt(None), Some(1), false), "no pid to compare");
        assert!(
            !worth_trying(&rt(Some(1)), Some(1), false),
            "the stale one while the start may still replace it"
        );
        assert!(
            worth_trying(&rt(Some(1)), Some(1), true),
            "the start exited (the mutex: the old runtime is alive), so retry it"
        );
    }

    #[test]
    fn a_stopped_home_is_not_started() {
        let dir = std::env::temp_dir().join(format!("sonara-hook-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!stopped(&dir));
        std::fs::write(dir.join(STOPPED), b"").unwrap();
        assert!(stopped(&dir));
        // A start is not even tried: a missing exe would be Dropped at once
        // too, so use an exe path that would fail loudly if spawned.
        let t = Instant::now();
        let d = deliver(
            &dir,
            &[json!({"type": "stream"})],
            Some(&dir.join("no-such-sonarad.exe")),
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
