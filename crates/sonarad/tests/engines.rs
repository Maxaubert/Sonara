//! The external engine messages of protocol 1.2 (spec 10), in process: a
//! `Server` with `with_engines` over the fake engine, and a small local
//! HTTP server standing in for an OpenAI-compatible provider. Each message's
//! success and every error row of spec 10.3.
use serde_json::{json, Map, Value};
use sonara_audio::{OutputCall, TestOutput};
use sonara_engine::external::keys::{KeyStore, MemoryStore};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, LicenseClass};
use sonara_reader::{Config, ReaderHandle, Registry};
use sonarad::engines::{self, Engines};
use sonarad::lifetime::Lifetime;
use sonarad::protocol::{Server, Session};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SECRET: &str = "sk-test-secret-0123456789abcdef";

/// One request the provider saw: path, headers (lower-case names), body.
type Seen = (String, Vec<(String, String)>, Vec<u8>);
/// Headers and body of a request.
type Request = (Vec<(String, String)>, Vec<u8>);

/// A provider: every POST answers `answer`; requests are kept.
#[derive(Clone)]
struct Provider {
    url: String,
    answer: Arc<Mutex<(u16, &'static str, Vec<u8>)>>,
    /// How long a speech reply waits before it is sent.
    delay: Arc<Mutex<Duration>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn wav(samples: usize) -> Vec<u8> {
    sonara_engine::wav::encode(&sonara_engine::PcmChunk {
        samples: vec![1000; samples],
        sample_rate: 24_000,
        channels: 1,
    })
}

impl Provider {
    fn start() -> Provider {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = Provider {
            url: format!("http://{}/v1", listener.local_addr().unwrap()),
            answer: Arc::new(Mutex::new((200, "audio/wav", wav(2400)))),
            delay: Arc::new(Mutex::new(Duration::ZERO)),
            seen: Arc::new(Mutex::new(Vec::new())),
        };
        let q = p.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let q = q.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(conn.try_clone().unwrap());
                    let mut first = String::new();
                    r.read_line(&mut first).unwrap_or(0);
                    let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                    let mut headers = Vec::new();
                    let mut len = 0;
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
                            break;
                        }
                        if let Some((k, v)) = l.split_once(':') {
                            let k = k.trim().to_ascii_lowercase();
                            if k == "content-length" {
                                len = v.trim().parse().unwrap_or(0);
                            }
                            headers.push((k, v.trim().to_string()));
                        }
                    }
                    let mut body = vec![0; len];
                    let _ = r.read_exact(&mut body);
                    q.seen.lock().unwrap().push((path.clone(), headers, body));
                    let (status, ct, b) = if path.ends_with("/audio/voices") {
                        (
                            200,
                            "application/json",
                            br#"{"voices": ["af_heart", "am_echo"]}"#.to_vec(),
                        )
                    } else {
                        let delay = *q.delay.lock().unwrap();
                        std::thread::sleep(delay);
                        q.answer.lock().unwrap().clone()
                    };
                    let mut out = conn;
                    let _ = write!(
                        out,
                        "HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        b.len()
                    );
                    let _ = out.write_all(&b);
                });
            }
        });
        p
    }

    fn fail(&self, status: u16, body: &str) {
        *self.answer.lock().unwrap() = (status, "application/json", body.as_bytes().to_vec());
    }

    fn speech_requests(&self) -> Vec<Request> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _, _)| p.ends_with("/audio/speech"))
            .map(|(_, h, b)| (h.clone(), b.clone()))
            .collect()
    }
}

struct Rig {
    server: Server,
    out: TestOutput,
    session: Session,
    home: PathBuf,
    keys: Arc<MemoryStore>,
    lines: Arc<Mutex<Vec<String>>>,
    provider: Provider,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn rig(tag: &str, external: bool) -> Rig {
    let home = std::env::temp_dir().join(format!("sonarad-engines-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let registry = Arc::new(Registry::new(&[
        LicenseClass::Permissive,
        LicenseClass::Os,
        LicenseClass::External,
    ]));
    let fake: Arc<dyn Engine> = Arc::new(FakeEngine::new());
    registry.register(fake.clone()).unwrap();
    let keys = Arc::new(MemoryStore::new());
    let lines = Arc::new(Mutex::new(Vec::new()));
    let l = lines.clone();
    let log: sonara_system::LogFn = Arc::new(move |s: &str| l.lock().unwrap().push(s.to_string()));
    let (e, _) = Engines::load(engines::Setup {
        home: home.clone(),
        store: keys.clone(),
        fallback: Some(fake),
        fallback_voice: String::new(),
        default_engine: "fake".into(),
        log: Some(log.clone()),
    });
    e.attach(registry.clone());
    let (out, rx) = TestOutput::new();
    let reader =
        ReaderHandle::new(Config::new(registry).with_output(Box::new(out.clone()), rx)).unwrap();
    let mut server = Server::new(
        reader,
        "secret".into(),
        Lifetime::new(Duration::from_secs(30), false),
    )
    .with_log(log);
    if external {
        server = server.with_engines(e);
    }
    let mut session = Session::tcp();
    let o = server.handle(&mut session, &json!({"type": "hello", "token": "secret"}));
    assert_eq!(o.reply["ok"], true);
    Rig {
        server,
        out,
        session,
        home,
        keys,
        lines,
        provider: Provider::start(),
    }
}

impl Rig {
    fn call(&mut self, req: Value) -> Value {
        self.server.handle(&mut self.session, &req).reply
    }

    fn profile(&self, id: &str) -> Value {
        json!({"id": id, "kind": "openai-compatible", "label": "Local",
            "url": self.provider.url, "key_ref": "credman",
            "options": {"preset": "kokoro-fastapi", "timeout_ms": 5000}})
    }

    fn add(&mut self, id: &str) -> Value {
        let p = self.profile(id);
        self.call(json!({"type": "engine_add", "engine": p, "secret": SECRET}))
    }
}

fn code(r: &Value) -> &str {
    r["error"]["code"].as_str().unwrap_or("")
}

#[test]
fn the_capability_and_the_list() {
    let mut r = rig("list", true);
    let o = r.call(json!({"type": "hello", "token": "secret"}));
    assert!(o["capabilities"]
        .as_array()
        .unwrap()
        .contains(&json!("engines")));
    assert_eq!(o["protocol"]["minor"], 2);
    assert!(r.server.capabilities().contains(&"engines"));
    let l = r.call(json!({"type": "engine_list"}));
    assert_eq!(l["ok"], true);
    assert_eq!(l["engines"], json!([]));
    assert_eq!(l["builtin"], json!(["fake"]));
    assert_eq!(l["kinds"], json!(["openai-compatible"]));
    assert!(l["presets"]
        .as_array()
        .unwrap()
        .contains(&json!("kokoro-fastapi")));
}

#[test]
fn a_host_without_external_engines_refuses_every_engine_message() {
    let mut r = rig("refuse", false);
    assert!(!r.server.capabilities().contains(&"engines"));
    for t in [
        "engine_list",
        "engine_add",
        "engine_remove",
        "engine_key",
        "engine_test",
    ] {
        let o = r.call(json!({"type": t, "engine": "x"}));
        assert_eq!(code(&o), "E_UNSUPPORTED", "{t}");
        assert_eq!(
            o["error"]["message"],
            "this runtime does not allow external engines"
        );
    }
}

#[test]
fn add_list_and_remove_round_trip() {
    let mut r = rig("roundtrip", true);
    let o = r.add("local");
    assert_eq!(o["ok"], true, "{o}");
    let v = &o["engine"];
    assert_eq!(v["id"], "local");
    assert_eq!(v["key_present"], true);
    assert_eq!(v["sends_text_to"], "127.0.0.1");
    assert_eq!(v["local"], true);
    assert_eq!(v["license_class"], "external");
    assert_eq!(v["current"], false, "adding does not select it");
    assert_eq!(v["status"]["status"], "ready");
    assert_eq!(r.keys.get("local").unwrap().unwrap().expose(), SECRET);
    // An existing id needs replace.
    let p = r.profile("local");
    let o = r.call(json!({"type": "engine_add", "engine": p}));
    assert_eq!(code(&o), "E_BAD_REQUEST");
    assert_eq!(
        o["error"]["message"],
        "engine 'local' exists; send replace: true"
    );
    let mut p = r.profile("local");
    p["model"] = json!("kokoro-2");
    let o = r.call(json!({"type": "engine_add", "engine": p, "replace": true}));
    assert_eq!(o["engine"]["model"], "kokoro-2");
    assert_eq!(o["engine"]["key_present"], true, "a replace keeps the key");
    let l = r.call(json!({"type": "engine_list"}));
    assert_eq!(l["engines"].as_array().unwrap().len(), 1);
    // Validation is E_BAD_REQUEST.
    let o = r
        .call(json!({"type": "engine_add", "engine": {"id": "Bad!", "kind": "openai-compatible"}}));
    assert_eq!(code(&o), "E_BAD_REQUEST");
    let o = r.call(json!({"type": "engine_add"}));
    assert_eq!(code(&o), "E_BAD_REQUEST");
    let o = r.call(json!({"type": "engine_remove", "engine": "local"}));
    assert_eq!(o["removed"], "local");
    assert_eq!(o["engine"], "fake");
    assert!(
        r.keys.get("local").unwrap().is_none(),
        "forget_key defaults to true"
    );
    let o = r.call(json!({"type": "engine_remove", "engine": "local"}));
    assert_eq!(code(&o), "E_NOT_FOUND");
}

#[test]
fn engine_key_rules() {
    let mut r = rig("key", true);
    r.add("local");
    let o = r.call(json!({"type": "engine_key", "engine": "local", "secret": null}));
    assert_eq!(o["key_present"], false);
    let o = r.call(json!({"type": "engine_key", "engine": "local", "secret": "sk-other-1234"}));
    assert_eq!(
        (o["engine"].clone(), o["key_present"].clone()),
        (json!("local"), json!(true))
    );
    let o = r.call(json!({"type": "engine_key", "engine": "nope", "secret": "x"}));
    assert_eq!(code(&o), "E_NOT_FOUND");
    let mut env = r.profile("envy");
    env["key_ref"] = json!("env:SONARA_TEST_NO_SUCH_KEY");
    assert_eq!(
        r.call(json!({"type": "engine_add", "engine": env}))["ok"],
        true
    );
    let o = r.call(json!({"type": "engine_key", "engine": "envy", "secret": "x"}));
    assert_eq!(code(&o), "E_BAD_REQUEST");
    assert_eq!(
        o["error"]["message"],
        "engine 'envy' reads its key from the environment variable SONARA_TEST_NO_SUCH_KEY"
    );
    let o = r.call(json!({"type": "engine_key", "engine": "local"}));
    assert_eq!(
        code(&o),
        "E_BAD_REQUEST",
        "secret is required (null deletes)"
    );
}

#[test]
fn engine_test_plays_and_reports_failures_with_a_reason() {
    let mut r = rig("test", true);
    r.add("local");
    let o = r.call(json!({"type": "engine_test", "engine": "local", "voice": "af_sky"}));
    assert_eq!(o["ok"], true, "{o}");
    assert_eq!(o["engine"], "local");
    assert_eq!(o["voice"], "af_sky");
    assert_eq!(o["sample_rate"], 24_000);
    assert_eq!(o["duration_ms"], 100);
    assert!(o["ms"].is_u64());
    let (headers, body) = r.provider.speech_requests().pop().unwrap();
    assert!(headers.contains(&("authorization".into(), format!("Bearer {SECRET}"))));
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body["input"],
        "Hello. This is how Sonara sounds with this voice."
    );
    let end = Instant::now() + Duration::from_secs(5);
    while !r
        .out
        .take_calls()
        .iter()
        .any(|c| matches!(c, OutputCall::PlayClip { .. }))
    {
        assert!(Instant::now() < end, "the test was played as a clip");
        std::thread::sleep(Duration::from_millis(5));
    }
    r.provider.fail(
        401,
        r#"{"error": {"message": "Incorrect API key provided"}}"#,
    );
    let o = r.call(json!({"type": "engine_test", "engine": "local", "play": false}));
    assert_eq!(code(&o), "E_ENGINE");
    assert_eq!(o["error"]["reason"], "auth");
    assert_eq!(
        o["error"]["message"],
        "Local refused the key (401): Incorrect API key provided"
    );
    let o = r.call(json!({"type": "engine_test", "engine": "nope"}));
    assert_eq!(code(&o), "E_NOT_FOUND");
    let o = r.call(json!({"type": "engine_test", "engine": "local", "text": "x".repeat(301)}));
    assert_eq!(code(&o), "E_BAD_REQUEST");
}

#[test]
fn a_slow_engine_test_does_not_hold_up_speech() {
    let mut r = rig("test-slow", true);
    r.add("local");
    *r.provider.delay.lock().unwrap() = Duration::from_secs(3);
    let server = &r.server;
    std::thread::scope(|s| {
        let tester = s.spawn(move || {
            let mut session = Session::tcp();
            server.handle(&mut session, &json!({"type": "hello", "token": "secret"}));
            server
                .handle(
                    &mut session,
                    &json!({"type": "engine_test", "engine": "local"}),
                )
                .reply
        });
        // Let the test reach the provider before speaking.
        let end = Instant::now() + Duration::from_secs(5);
        while r.provider.speech_requests().is_empty() {
            assert!(Instant::now() < end, "the test never reached the provider");
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut session = Session::tcp();
        server.handle(&mut session, &json!({"type": "hello", "token": "secret"}));
        let start = Instant::now();
        let o = server
            .handle(&mut session, &json!({"type": "speak", "text": "Hello."}))
            .reply;
        assert_eq!(o["ok"], true, "{o}");
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "speak waited {:?} for the engine test",
            start.elapsed()
        );
        let o = tester.join().unwrap();
        assert_eq!(o["ok"], true, "{o}");
    });
}

#[test]
fn engine_remove_of_the_current_engine_switches_first() {
    let mut r = rig("remove-current", true);
    r.add("local");
    let o = r.call(json!({"type": "set", "key": "engine", "value": "local"}));
    assert_eq!(o["value"], "local");
    assert_eq!(r.server.store().user("engine"), Some(json!("local")));
    let l = r.call(json!({"type": "engine_list"}));
    assert_eq!(l["engines"][0]["current"], true);
    let o = r.call(json!({"type": "engine_remove", "engine": "local", "forget_key": false}));
    assert_eq!(o["engine"], "fake");
    assert_eq!(
        r.call(json!({"type": "get", "key": "engine"}))["value"],
        "fake"
    );
    assert_eq!(
        r.server.store().user("engine"),
        Some(json!("fake")),
        "saved like any set"
    );
    assert!(
        r.keys.get("local").unwrap().is_some(),
        "forget_key false keeps it"
    );
}

#[test]
fn speech_on_a_profile_reaches_the_provider_and_falls_back_with_a_reason() {
    let mut r = rig("speak", true);
    r.add("local");
    r.call(json!({"type": "set", "key": "engine", "value": "local"}));
    r.call(json!({"type": "speak", "text": "Hello there."}));
    let end = Instant::now() + Duration::from_secs(5);
    while r.provider.speech_requests().is_empty() {
        assert!(Instant::now() < end, "no request reached the provider");
        std::thread::sleep(Duration::from_millis(5));
    }
    // A provider that refuses the key: the reason shows in engine_status.
    r.provider.fail(401, r#"{"error": {"message": "nope"}}"#);
    r.call(json!({"type": "speak", "text": "Second item.", "interrupt": true}));
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let l = r.call(json!({"type": "engine_list"}));
        if l["engines"][0]["status"]["reason"] == "auth" {
            assert_eq!(l["engines"][0]["status"]["status"], "unavailable");
            assert_eq!(l["engines"][0]["status"]["fallback"], "fake");
            break;
        }
        assert!(Instant::now() < end, "no auth reason: {l}");
        std::thread::sleep(Duration::from_millis(10));
    }
    let lines = r.lines.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("engine local fallback reason=auth status=401 -> fake:")),
        "{lines:?}"
    );
}

#[test]
fn voices_refresh_lists_the_servers_voices() {
    let mut r = rig("voices", true);
    r.add("local");
    let o = r.call(json!({"type": "voices", "engine": "local", "refresh": true}));
    let ids: Vec<&str> = o["voices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["af_heart", "am_echo"]);
    assert_eq!(o["voices"][0]["license_class"], "external");
    // The engine accepts a voice it does not list.
    r.call(json!({"type": "set", "key": "engine", "value": "local"}));
    let o = r.call(json!({"type": "set", "key": "voice", "value": "my-clone"}));
    assert_eq!(o["value"], "my-clone");
}

#[test]
fn secret_never_in_trace_log_or_engines_json() {
    sonarad::trace_log::set_debug(true);
    let mut r = rig("secret", true);
    let mut p = r.profile("local");
    p["api_key"] = json!(SECRET);
    let o = r.call(json!({"type": "engine_add", "engine": p, "secret": SECRET}));
    assert_eq!(o["ok"], true);
    r.call(json!({"type": "engine_key", "engine": "local", "secret": SECRET}));
    r.call(json!({"type": "engine_test", "engine": "local", "play": false}));
    let all: Vec<Value> = vec![o.clone(), r.call(json!({"type": "engine_list"}))];
    for v in &all {
        assert!(!v.to_string().contains(SECRET), "a reply: {v}");
    }
    let lines = r.lines.lock().unwrap().clone();
    assert!(lines
        .iter()
        .any(|l| l.starts_with("in ") && l.contains("engine_add")));
    assert!(lines.iter().any(|l| l == "engine key id=local set"));
    for l in &lines {
        assert!(!l.contains(SECRET), "a log line: {l}");
        assert!(
            !l.contains("\"secret\""),
            "the field is dropped, not masked: {l}"
        );
    }
    let file = std::fs::read_to_string(r.home.join("engines.json")).unwrap();
    assert!(
        !file.contains(SECRET) && !file.contains("api_key"),
        "{file}"
    );
    let stored: Map<String, Value> = serde_json::from_str::<Value>(&file).unwrap()["engines"][0]
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(stored["key_ref"], "credman");
}
