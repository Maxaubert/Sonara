//! The troubleshooting lines of #219 from the built `sonarad.exe`
//! (`--engine fake --system fake`): what came in, what the agent decided
//! (spoken, held, not spoken and why), the exact text read with its kind
//! and message, text dropped unread with the reason, and with the setting
//! `debug_log` off none of the text.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);

/// A temp home and the runtime on it; killed and removed when dropped.
struct Runtime {
    home: PathBuf,
    child: Child,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn start(tag: &str) -> Runtime {
    let home = std::env::temp_dir().join(format!("sonarad-trace-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("fake-system.json"), "{}").unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_sonarad"))
        .arg("--home")
        .arg(&home)
        .args(["--engine", "fake", "--system", "fake", "--idle-exit", "60"])
        .env_remove("SONARA_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start sonarad");
    Runtime { home, child }
}

fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let end = Instant::now() + TIMEOUT;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() > end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Client {
    w: TcpStream,
    r: BufReader<TcpStream>,
}

impl Client {
    fn connect(rt: &Runtime) -> Client {
        let pid = rt.child.id();
        let info: Value = wait_for(|| {
            let text = std::fs::read_to_string(rt.home.join("runtime.json")).ok()?;
            let v: Value = serde_json::from_str(&text).ok()?;
            (v["pid"] == pid).then_some(v)
        })
        .expect("sonarad wrote no runtime.json");
        let port = info["port"].as_u64().unwrap() as u16;
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(TIMEOUT)).unwrap();
        let mut c = Client {
            r: BufReader::new(s.try_clone().unwrap()),
            w: s,
        };
        c.request(json!({
            "type": "hello",
            "token": info["token"],
            "client": {"name": "trace-log-test", "version": "1"},
            "extensions": ["agent", "system"],
        }));
        c
    }

    fn request(&mut self, msg: Value) -> Value {
        self.w
            .write_all(format!("{msg}\n").as_bytes())
            .expect("send");
        let mut line = String::new();
        self.r.read_line(&mut line).expect("reply");
        let v: Value = serde_json::from_str(&line).expect("json reply");
        assert_eq!(v["ok"], true, "{msg} -> {v}");
        v
    }
}

fn log_text(home: &Path) -> String {
    std::fs::read_to_string(home.join("logs").join("sonarad.log")).unwrap_or_default()
}

fn wait_log(rt: &Runtime, what: &str, ok: impl Fn(&str) -> bool) -> String {
    wait_for(|| {
        let t = log_text(&rt.home);
        ok(&t).then_some(t)
    })
    .unwrap_or_else(|| panic!("no {what} in the log:\n{}", log_text(&rt.home)))
}

fn has_line(log: &str, ok: impl Fn(&str) -> bool) -> bool {
    log.lines()
        .any(|l| ok(l.split_once(' ').map_or("", |(_, rest)| rest)))
}

#[test]
fn what_came_in_what_was_decided_read_and_dropped_is_in_the_log() {
    let rt = start("on");
    let mut c = Client::connect(&rt);
    for m in [
        json!({"type": "channel_open", "channel": "c1", "label": "proj"}),
        json!({"type": "focus", "channel": "c1"}),
        json!({"type": "turn_start", "channel": "c1", "t": 100.0}),
        json!({"type": "stream", "channel": "c1", "delta": "Alpha one. Alpha two.", "index": 0, "final": true, "t": 101.0}),
        json!({"type": "turn_end", "channel": "c1", "t": 101.0}),
        json!({"type": "ask", "channel": "c1", "kind": "question", "text": "Which colour?", "options": [{"label": "Red"}, {"label": "Blue"}], "t": 101.0}),
        json!({"type": "ask", "channel": "c1", "kind": "permission", "text": "Claude needs your permission", "t": 101.0}),
    ] {
        c.request(m);
    }
    let log = wait_log(&rt, "the question read", |t| {
        has_line(t, |l| {
            l.starts_with("read text") && l.contains("kind=question")
        })
    });
    // What came in, compact, with the option labels.
    assert!(
        has_line(&log, |l| l.starts_with("in {")
            && l.contains(r#""type":"stream""#)
            && l.contains("Alpha one.")),
        "{log}"
    );
    assert!(
        has_line(&log, |l| l.starts_with("in {")
            && l.contains(r#""options":["Red","Blue"]"#)),
        "{log}"
    );
    assert!(!log.contains(r#""token""#), "never the token: {log}");
    // What the agent decided.
    assert!(
        has_line(&log, |l| l
            .starts_with("agent stream channel=c1 prose: held: 2 chunk(s)")),
        "{log}"
    );
    assert!(
        has_line(&log, |l| l
            .starts_with("agent turn_end channel=c1 speak kind=prose entry=")
            && l.ends_with("text=\"Alpha one.\"")),
        "{log}"
    );
    assert!(
        has_line(&log, |l| l == "agent turn_end earcon turn_done"),
        "{log}"
    );
    assert!(
        has_line(&log, |l| l.starts_with(
            "agent \"ask permission\" channel=c1 permission: not spoken"
        ) && l.contains("(#11)")),
        "{log}"
    );
    // What was read: the text, its kind and the message behind it.
    assert!(
        has_line(&log, |l| l.starts_with("read text item=")
            && l.contains(
                "session=proj kind=prose from=turn_end chunks=1/1 text=\"Alpha one.\""
            )),
        "{log}"
    );
    assert!(
        has_line(&log, |l| l.starts_with("read text item=")
            && l.contains("kind=question from=\"ask question\"")
            && l.contains("Which colour?")
            && l.contains("Blue")),
        "{log}"
    );
    // Late text of the turn before, and text dropped by a new turn.
    c.request(json!({"type": "turn_start", "channel": "c1", "t": 200.0}));
    c.request(json!({"type": "stream", "channel": "c1", "delta": "Too late.", "index": 0, "final": true, "t": 150.0}));
    c.request(json!({"type": "stream", "channel": "c1", "delta": "Beta one. Beta two. Beta three. Beta four. Beta five. Beta six.", "index": 0, "final": true, "t": 201.0}));
    c.request(json!({"type": "turn_start", "channel": "c1", "t": 300.0}));
    c.request(json!({"type": "set", "key": "mute_level", "value": 1}));
    let log = wait_log(&rt, "the mute wipe", |t| {
        has_line(t, |l| l == "agent mute_level wipe reason=mute")
    });
    assert!(
        has_line(&log, |l| l
            .starts_with("agent stream channel=c1 dropped: late text")),
        "{log}"
    );
    assert!(
        has_line(&log, |l| l
            == "agent turn_start channel=c1 wipe reason=turn_start"),
        "{log}"
    );
    assert!(
        has_line(&log, |l| l.starts_with("drop channel=c1 entry=")
            && l.contains(
                "kind=prose from=stream reason=turn_start text=\"Beta"
            )),
        "{log}"
    );
    assert!(
        !has_line(&log, |l| !l.starts_with("in {") && l.contains("Too late.")),
        "late text only came in, it never reached a channel: {log}"
    );
}

#[test]
fn with_debug_log_off_no_text_is_written_and_the_choice_is_saved() {
    let rt = start("off");
    let mut c = Client::connect(&rt);
    let v = c.request(json!({"type": "set", "key": "debug_log", "value": false}));
    assert_eq!(v["value"], false);
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(rt.home.join("config.json")).unwrap())
            .unwrap();
    assert_eq!(saved["debug_log"], false);
    for m in [
        json!({"type": "channel_open", "channel": "c1", "label": "proj"}),
        json!({"type": "stream", "channel": "c1", "delta": "Secret prose.", "index": 0, "final": true}),
        json!({"type": "turn_end", "channel": "c1"}),
    ] {
        c.request(m);
    }
    let log = wait_log(&rt, "the read text line", |t| {
        has_line(t, |l| {
            l.starts_with("read text") && l.contains("kind=prose")
        })
    });
    assert!(!log.contains("ecret"), "{log}");
    assert!(
        has_line(&log, |l| l.starts_with("in {")
            && l.contains(r#""type":"stream""#)),
        "the message is still logged, without its text: {log}"
    );
    assert_eq!(
        c.request(json!({"type": "get", "key": "debug_log"}))["value"],
        false
    );
    let v = c.request(json!({"type": "set", "key": "debug_log", "value": true}));
    assert_eq!(v["value"], true);
}

#[test]
fn what_a_connection_sends_before_hello_never_reaches_the_log() {
    let rt = start("unauthed");
    // An authed client first, so the runtime is up and `runtime.json` read.
    let mut c = Client::connect(&rt);
    let info: Value =
        serde_json::from_str(&std::fs::read_to_string(rt.home.join("runtime.json")).unwrap())
            .unwrap();
    let port = info["port"].as_u64().unwrap() as u16;
    for _ in 0..5 {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(TIMEOUT)).unwrap();
        let msg =
            json!({"type": "ask", "channel": "x", "kind": "question", "text": "Intruder text"});
        s.write_all(format!("{msg}\n").as_bytes()).unwrap();
        let mut line = String::new();
        let _ = BufReader::new(s).read_line(&mut line);
        assert!(line.contains("E_AUTH"), "{line}");
    }
    c.request(json!({"type": "channel_open", "channel": "c1", "label": "marker"}));
    let log = wait_log(&rt, "the marker", |t| t.contains("\"marker\""));
    assert!(!log.contains("Intruder"), "{log}");
    let refusals = log.lines().filter(|l| l.contains("E_AUTH")).count();
    assert!(
        refusals <= 1,
        "refusals before hello are rate limited:\n{log}"
    );
}
