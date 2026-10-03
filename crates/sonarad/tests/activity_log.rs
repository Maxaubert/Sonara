//! The built `sonarad.exe` (`--engine fake --system fake`) writes the
//! activity lines of #217 to `logs\sonarad.log`: an item read in audio mode
//! `pause` with a playing app gives `read start`, `media pause` (naming the
//! item), `read end` and `media resume`, in that order, and with the
//! setting `debug_log` off never the spoken text (#219).
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

fn start(tag: &str, world: Value) -> Runtime {
    let home = std::env::temp_dir().join(format!("sonarad-activity-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("fake-system.json"), world.to_string()).unwrap();
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

fn runtime_info(rt: &Runtime) -> Value {
    let pid = rt.child.id();
    wait_for(|| {
        let text = std::fs::read_to_string(rt.home.join("runtime.json")).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        (v["pid"] == pid).then_some(v)
    })
    .expect("sonarad wrote no runtime.json")
}

struct Client {
    w: TcpStream,
    r: BufReader<TcpStream>,
}

impl Client {
    fn connect(info: &Value) -> Client {
        let port = info["port"].as_u64().unwrap() as u16;
        let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(TIMEOUT)).unwrap();
        Client {
            r: BufReader::new(s.try_clone().unwrap()),
            w: s,
        }
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

/// The index of the first line starting with `prefix` (after the
/// timestamp).
fn find(lines: &[&str], prefix: &str) -> usize {
    lines
        .iter()
        .position(|l| {
            l.split_once(' ')
                .is_some_and(|(_, rest)| rest.starts_with(prefix))
        })
        .unwrap_or_else(|| panic!("no '{prefix}' line in {lines:#?}"))
}

#[test]
fn reading_and_media_pause_lines_appear_in_order() {
    let rt = start(
        "pause",
        json!({"media": [{"app": "spotify", "playing": true}]}),
    );
    let info = runtime_info(&rt);
    let mut c = Client::connect(&info);
    c.request(json!({
        "type": "hello",
        "token": info["token"],
        "client": {"name": "activity-log-test", "version": "1"},
        "extensions": ["system"],
    }));
    c.request(json!({"type": "set", "key": "audio_mode", "value": "pause"}));
    c.request(json!({"type": "set", "key": "debug_log", "value": false}));
    let item = c.request(json!({
        "type": "speak",
        "text": "Private words. More private words.",
        "label": "my session",
    }))["item_id"]
        .as_u64()
        .unwrap();
    let text = wait_for(|| {
        let t = log_text(&rt.home);
        t.contains("media resume").then_some(t)
    })
    .unwrap_or_else(|| panic!("no media resume line: {}", log_text(&rt.home)));
    let lines: Vec<&str> = text.lines().collect();
    let start = find(&lines, &format!("read start item={item} session="));
    let end = find(&lines, &format!("read end item={item} finished"));
    let pause = find(&lines, "media pause apps=spotify (reason: reading item=");
    let resume = find(&lines, "media resume apps=spotify (reason: idle)");
    assert!(start < end, "{lines:#?}");
    assert!(pause < resume, "{lines:#?}");
    assert!(end < resume, "resumed after the idle grace: {lines:#?}");
    assert!(
        lines[pause].contains(&format!("item={item}")),
        "the pause names the item read: {}",
        lines[pause]
    );
    assert!(!text.contains("rivate"), "never the spoken text: {text}");
    // Every line starts with a UTC timestamp.
    assert!(
        lines.iter().all(|l| l
            .split_once(' ')
            .is_some_and(|(t, _)| t.len() == 24 && t.ends_with('Z'))),
        "{lines:#?}"
    );
}
