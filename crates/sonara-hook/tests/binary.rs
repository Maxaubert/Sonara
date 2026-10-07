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
        // Never start a real runtime from a unit test (conformance covers
        // the start with the fake engine and system).
        .env("SONARA_NO_START", "1")
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
    assert_eq!(got[0]["extensions"], json!(["agent", "system"]));
    assert_eq!(got[0]["keep_alive"], true);
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
    let t = Instant::now();
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
    assert!(
        t.elapsed() < Duration::from_secs(8),
        "a dead runtime does not hold the hooks up: {:?}",
        t.elapsed()
    );
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

// -- the troubleshooting log (#219) -----------------------------------------

fn hook_log(home: &Home) -> String {
    std::fs::read_to_string(home.dir.join("logs").join("hook.log")).unwrap_or_default()
}

#[test]
fn every_invocation_logs_its_payload_its_messages_and_the_delivery() {
    let home = Home::new("log");
    let rx = fake_runtime(&home);
    let payload = json!({
        "session_id": "s9",
        "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "Which colour?", "options": [{"label": "Red"}, {"label": "Blue"}]}]},
    });
    assert_eq!(
        run(&home, "PreToolUse", payload.to_string().as_bytes(), &[]),
        0
    );
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let silent = br#"{"session_id": "s9", "notification_type": "idle_prompt", "message": "Claude is waiting for your input"}"#;
    assert_eq!(run(&home, "Notification", silent, &[]), 0);
    let log = hook_log(&home);
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    let ask = lines[0];
    assert!(ask.contains(" hook PreToolUse pid="), "{ask}");
    assert!(ask.contains(" sent ms="), "{ask}");
    assert!(ask.contains("session=s9 tool=\"AskUserQuestion\""), "{ask}");
    assert!(
        ask.contains(r#""question":"Which colour?""#),
        "the payload: {ask}"
    );
    assert!(
        ask.contains(r#""type":"ask""#) && ask.contains(r#""label":"Blue""#),
        "{ask}"
    );
    let note = lines[1];
    assert!(
        note.contains("hook Notification") && note.contains("nothing to send"),
        "{note}"
    );
    assert!(note.contains("notification=\"idle_prompt\""), "{note}");
    assert!(note.contains("Claude is waiting for your input"), "{note}");
}

#[test]
fn a_huge_field_is_clipped_and_debug_log_off_keeps_text_out() {
    let home = Home::new("log-clip");
    let _rx = fake_runtime(&home);
    let big = "w".repeat(20_000);
    let question =
        json!({"questions": [{"question": big, "header": "Pick", "options": [{"label": "Red"}]}]});
    let payload =
        json!({"session_id": "s1", "tool_name": "AskUserQuestion", "tool_input": question});
    assert_eq!(
        run(&home, "PreToolUse", payload.to_string().as_bytes(), &[]),
        0
    );
    let log = hook_log(&home);
    assert!(log.contains("...[+15904 bytes]"), "clipped at 4 KB");
    assert!(
        log.contains(r#""header":"Pick""#),
        "short fields stay whole"
    );
    assert!(log.len() < 20_000, "{}", log.len());
    // Another tool's input (a file being written) keeps its field names only.
    let write = json!({"session_id": "s1", "tool_name": "Write", "tool_input": {"file_path": "a.txt", "content": "private file"}});
    assert_eq!(
        run(&home, "PreToolUse", write.to_string().as_bytes(), &[]),
        0
    );
    let last = hook_log(&home).lines().last().unwrap().to_string();
    assert!(
        last.contains(r#""fields":["content","file_path"]"#),
        "{last}"
    );
    assert!(!last.contains("private file"), "{last}");
    std::fs::write(home.dir.join("config.json"), r#"{"debug_log": false}"#).unwrap();
    let secret = json!({"session_id": "s1", "notification_type": "permission_prompt", "message": "Secret words"});
    assert_eq!(
        run(&home, "Notification", secret.to_string().as_bytes(), &[]),
        0
    );
    let last = hook_log(&home).lines().last().unwrap().to_string();
    assert!(
        last.contains("hook Notification") && last.contains(r#"sent=["ask"]"#),
        "{last}"
    );
    assert!(!last.contains("Secret"), "{last}");
}

#[test]
fn a_log_that_cannot_be_written_never_fails_or_holds_up_the_hook() {
    // `logs` is a file: no folder can be made there.
    let home = Home::new("log-broken");
    let rx = fake_runtime(&home);
    std::fs::write(home.dir.join("logs"), b"not a folder").unwrap();
    let payload = br#"{"session_id": "s1"}"#;
    assert_eq!(run(&home, "Stop", payload, &[]), 0);
    assert!(
        rx.recv_timeout(Duration::from_secs(10)).is_ok(),
        "delivered"
    );
    // A log locked by another writer: the hook skips its line in time.
    let home = Home::new("log-locked");
    let rx = fake_runtime(&home);
    std::fs::create_dir_all(home.dir.join("logs")).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(home.dir.join("logs").join(".lock"))
        .unwrap();
    lock.lock().unwrap();
    let t = Instant::now();
    assert_eq!(run(&home, "Stop", payload, &[]), 0);
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert!(
        rx.recv_timeout(Duration::from_secs(10)).is_ok(),
        "delivered"
    );
    assert_eq!(hook_log(&home), "", "skipped while locked");
    lock.unlock().unwrap();
    assert_eq!(run(&home, "Stop", payload, &[]), 0);
    assert!(hook_log(&home).contains("hook Stop"));
}

// -- a question whose message has no text (#283) ---------------------------

#[test]
fn a_question_without_text_sends_its_thinking_first() {
    // Transcript rows 4834 to 4836 (2026-10-07): only thinking, then the
    // AskUserQuestion. The built hook reads the transcript and sends the
    // last thinking as one final stream before the asks.
    let home = Home::new("thinking");
    let rx = fake_runtime(&home);
    let transcript = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/transcripts/thinking_only_question.jsonl");
    let payload = json!({
        "session_id": "s1",
        "transcript_path": transcript,
        "tool_use_id": "toolu_01EXJLaWWqDBnjEJ38tJ3meY",
        "hook_event_name": "PreToolUse",
        "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [
            {"question": "Where do the Norwegian names live?", "options": [{"label": "A"}]},
            {"question": "How is the price shown?", "options": [{"label": "B"}]}
        ]}
    });
    assert_eq!(run(&home, "PreToolUse", payload.to_string().as_bytes(), &[]), 0);
    let got = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let kinds: Vec<&str> = got.iter().map(|m| m["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["hello", "stream", "ask", "ask"]);
    assert!(got[1]["delta"]
        .as_str()
        .unwrap()
        .starts_with("The card is next."));
    assert_eq!(got[1]["final"], true);
    assert_eq!((got[2]["set_index"].clone(), got[3]["set_index"].clone()), (json!(0), json!(1)));
}
