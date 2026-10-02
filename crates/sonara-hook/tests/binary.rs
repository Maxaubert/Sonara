//! The built `sonara-hook.exe` against a fake runtime (a loopback listener
//! and a `runtime.json` in a temp `SONARA_HOME`): one connection, `hello`
//! first, every message stamped with `t`, exit code 0 whatever happens.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Home {
    dir: PathBuf,
}

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("sonara-hook-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Home { dir }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A runtime that answers every line with `ok` and reports what it got
/// (one Vec per connection).
fn fake_runtime(home: &Home) -> mpsc::Receiver<Vec<Value>> {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::fs::write(
        home.dir.join("runtime.json"),
        json!({"pid": 1, "port": port, "http_port": 1, "token": "tok"}).to_string(),
    )
    .unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            let mut w = s.try_clone().unwrap();
            let mut got = Vec::new();
            for line in BufReader::new(s).lines() {
                let Ok(line) = line else { break };
                got.push(serde_json::from_str::<Value>(&line).unwrap());
                let _ = w.write_all(b"{\"ok\": true}\n");
            }
            let _ = tx.send(got);
        }
    });
    rx
}

fn run(home: &Home, event: &str, stdin: &[u8], extra: &[(&str, &str)]) -> i32 {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sonara-hook"));
    cmd.arg(event)
        .env("SONARA_HOME", &home.dir)
        .env_remove("SONARA_SUMMARIZER")
        .env_remove("SONARA_CAPTURE")
        .env_remove("SONARA_HOST_TAB")
        .env_remove("PRISM_TAB_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in extra {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code().unwrap_or(-1);
        }
        assert!(Instant::now() < end, "the hook hung");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[test]
fn a_prompt_is_sent_as_one_batch_after_hello_and_stamped() {
    let home = Home::new("batch");
    let rx = fake_runtime(&home);
    let before = now();
    let payload = br#"{"session_id": "s1", "cwd": "C:\\work\\proj"}"#;
    let code = run(
        &home,
        "UserPromptSubmit",
        payload,
        &[("SONARA_HOST_TAB", "t-3")],
    );
    assert_eq!(code, 0);
    let got = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let kinds: Vec<&str> = got.iter().map(|m| m["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["hello", "channel_open", "focus", "turn_start"]);
    assert_eq!(got[0]["token"], "tok");
    assert_eq!(got[0]["extensions"], json!(["agent"]));
    assert_eq!(got[1]["label"], "proj");
    assert_eq!(got[1]["host_tab"], "t-3");
    let t = got[3]["t"].as_f64().unwrap();
    assert!(t >= before - 1.0 && t <= now(), "t = {t}");
    assert!(
        got[1..].iter().all(|m| m["t"] == got[3]["t"]),
        "one start time"
    );
}

#[test]
fn nothing_is_sent_for_silent_events_or_inside_the_summarizer() {
    let home = Home::new("silent");
    let rx = fake_runtime(&home);
    assert_eq!(
        run(
            &home,
            "Notification",
            br#"{"notification_type": "idle_prompt"}"#,
            &[]
        ),
        0
    );
    assert_eq!(
        run(
            &home,
            "Stop",
            br#"{"session_id": "s"}"#,
            &[("SONARA_SUMMARIZER", "1")]
        ),
        0
    );
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
}

#[test]
fn it_always_exits_zero() {
    let home = Home::new("errors");
    // No runtime.json, then a runtime.json naming a closed port, garbage
    // stdin, no event name.
    assert_eq!(run(&home, "Stop", br#"{"session_id": "s"}"#, &[]), 0);
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(
        home.dir.join("runtime.json"),
        json!({"port": port, "token": "t"}).to_string(),
    )
    .unwrap();
    assert_eq!(run(&home, "Stop", br#"{"session_id": "s"}"#, &[]), 0);
    assert_eq!(run(&home, "MessageDisplay", b"\xff not json", &[]), 0);
    assert_eq!(run(&home, "", b"", &[]), 0);
}

#[test]
fn the_raw_payload_can_be_captured() {
    let home = Home::new("capture");
    let cap = home.dir.join("cap");
    let code = run(
        &home,
        "Stop",
        br#"{"session_id": "raw"}"#,
        &[("SONARA_CAPTURE", cap.to_str().unwrap())],
    );
    assert_eq!(code, 0);
    let files: Vec<_> = std::fs::read_dir(&cap).unwrap().collect();
    assert_eq!(files.len(), 1);
    let f = files[0].as_ref().unwrap().path();
    assert!(f
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("Stop-"));
    assert_eq!(std::fs::read(f).unwrap(), br#"{"session_id": "raw"}"#);
}
