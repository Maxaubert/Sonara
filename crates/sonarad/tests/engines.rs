//! The external engine messages of protocol 1.2 (spec 10), in process: a
//! `Server` with `with_engines` over the fake engine, and a small local
//! HTTP server standing in for an OpenAI-compatible provider. Each message's
//! success and every error row of spec 10.3.
mod common;

use common::Provider;
use serde_json::{json, Map, Value};
use sonara_audio::{OutputCall, TestOutput};
use sonara_engine::external::keys::{KeyStore, MemoryStore, Secret};
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, LicenseClass};
use sonara_reader::{Config, ReaderHandle, Registry};
use sonarad::engines::{self, Engines};
use sonarad::lifetime::Lifetime;
use sonarad::protocol::{Server, Session};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SECRET: &str = "sk-test-secret-0123456789abcdef";

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
    assert_eq!(o["protocol"]["minor"], 4);
    assert!(r.server.capabilities().contains(&"engines"));
    let l = r.call(json!({"type": "engine_list"}));
    assert_eq!(l["ok"], true);
    assert_eq!(l["engines"], json!([]));
    assert_eq!(l["builtin"], json!(["fake"]));
    assert_eq!(
        l["kinds"],
        json!([
            "openai-compatible",
            "elevenlabs",
            "azure",
            "google",
            "gemini",
            "cartesia",
            "deepgram",
            "command"
        ])
    );
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
        "engine_reload",
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

/// Whether any request `p` saw carried a key header.
fn saw_a_key(p: &Provider) -> bool {
    p.seen.lock().unwrap().iter().any(|(_, h, _)| {
        h.iter().any(|(k, v)| {
            matches!(
                k.as_str(),
                "authorization" | "xi-api-key" | "x-goog-api-key" | "ocp-apim-subscription-key"
            ) || v.contains(SECRET)
        })
    })
}

/// Speak through `id` and wait until `p` saw a speech request (or the
/// fallback spoke).
fn speak_through(r: &mut Rig, id: &str) {
    r.call(json!({"type": "set", "key": "engine", "value": id}));
    r.call(json!({"type": "speak", "text": "Where does this go?", "interrupt": true}));
    let _ = r.call(json!({"type": "engine_test", "engine": id, "play": false}));
    let _ = r.call(json!({"type": "voices", "engine": id, "refresh": true}));
    std::thread::sleep(Duration::from_millis(300));
}

#[test]
fn replacing_the_url_never_sends_the_old_key_to_the_new_host() {
    for (tag, http) in [("rebind-tcp", false), ("rebind-http", true)] {
        let mut r = rig(tag, true);
        assert_eq!(r.add("local")["ok"], true);
        let other = Provider::start();
        let mut p = r.profile("local");
        p["url"] = json!(other.url);
        let req = json!({"type": "engine_add", "engine": p, "replace": true});
        let o = if http {
            let mut s = Session::http();
            r.server.handle(&mut s, &req).reply
        } else {
            r.call(req)
        };
        assert_eq!(o["ok"], true, "{o}");
        assert_eq!(
            o["engine"]["key_present"], false,
            "{tag}: the key was not entered for this address"
        );
        assert!(
            r.keys.get("local").unwrap().is_none(),
            "{tag}: an origin change without a new secret deletes the stored key"
        );
        speak_through(&mut r, "local");
        assert!(
            !saw_a_key(&other),
            "{tag}: the old key reached the new host"
        );
        let o = r.call(json!({"type": "engine_test", "engine": "local", "play": false}));
        assert_eq!(o["error"]["reason"], "no_key", "{tag}: {o}");
    }
}

#[test]
fn a_key_bound_elsewhere_is_never_sent() {
    // A key stored for one address (a stale credential, a profile replaced
    // by an older runtime) is refused for another, never sent.
    let mut r = rig("bound-elsewhere", true);
    let other = Provider::start();
    let mut p = r.profile("local");
    p["url"] = json!(other.url);
    assert_eq!(
        r.call(json!({"type": "engine_add", "engine": p}))["ok"],
        true
    );
    let first = r.provider.url.trim_end_matches("/v1").to_string();
    r.keys.set("local", &Secret::new(SECRET), &first).unwrap();
    let o = r.call(json!({"type": "engine_list"}));
    assert_eq!(o["engines"][0]["key_present"], false, "{o}");
    speak_through(&mut r, "local");
    assert!(!saw_a_key(&other));
    let o = r.call(json!({"type": "engine_test", "engine": "local", "play": false}));
    assert_eq!(o["error"]["reason"], "no_key", "{o}");
    assert!(
        o["error"]["message"]
            .as_str()
            .unwrap()
            .contains("enter the key again"),
        "{o}"
    );
    // A key without a recorded origin (written by an older runtime after
    // the migration) is refused too.
    r.keys.set_unbound("local", &Secret::new(SECRET));
    speak_through(&mut r, "local");
    assert!(!saw_a_key(&other));
}

#[test]
fn the_key_survives_changes_that_keep_the_origin() {
    let mut r = rig("same-origin", true);
    r.add("local");
    let mut p = r.profile("local");
    p["voice"] = json!("am_echo");
    p["model"] = json!("kokoro-2");
    p["label"] = json!("Renamed");
    // The same origin spelled differently (trailing slash, other path).
    p["url"] = json!(format!("{}/", r.provider.url));
    let o = r.call(json!({"type": "engine_add", "engine": p, "replace": true}));
    assert_eq!(o["engine"]["key_present"], true, "{o}");
    let o = r.call(json!({"type": "engine_test", "engine": "local", "play": false}));
    assert_eq!(o["ok"], true, "{o}");
    let (headers, _) = r.provider.speech_requests().pop().unwrap();
    assert!(headers.contains(&("authorization".into(), format!("Bearer {SECRET}"))));
}

#[test]
fn a_new_key_with_the_new_url_works() {
    let mut r = rig("new-key", true);
    r.add("local");
    let other = Provider::start();
    let mut p = r.profile("local");
    p["url"] = json!(other.url);
    let o = r.call(json!({"type": "engine_add", "engine": p, "replace": true,
        "secret": "sk-for-the-new-host-123"}));
    assert_eq!(o["engine"]["key_present"], true, "{o}");
    let o = r.call(json!({"type": "engine_test", "engine": "local", "play": false}));
    assert_eq!(o["ok"], true, "{o}");
    let (headers, _) = other.speech_requests().pop().unwrap();
    assert!(headers.contains(&(
        "authorization".into(),
        "Bearer sk-for-the-new-host-123".into()
    )));
    // Or with engine_key after the replace.
    let third = Provider::start();
    let mut p = r.profile("local");
    p["url"] = json!(third.url);
    r.call(json!({"type": "engine_add", "engine": p, "replace": true}));
    let o = r.call(json!({"type": "engine_key", "engine": "local", "secret": "sk-third-host-456"}));
    assert_eq!(o["key_present"], true, "{o}");
    let o = r.call(json!({"type": "engine_test", "engine": "local", "play": false}));
    assert_eq!(o["ok"], true, "{o}");
    let (headers, _) = third.speech_requests().pop().unwrap();
    assert!(headers.contains(&("authorization".into(), "Bearer sk-third-host-456".into())));
    assert!(!other
        .seen
        .lock()
        .unwrap()
        .iter()
        .any(|(_, h, _)| h.iter().any(|(_, v)| v.contains("sk-third-host-456"))));
}

#[test]
fn an_env_key_follows_only_a_locally_confirmed_url() {
    let var = "SONARA_TEST_ORIGIN_API_KEY";
    std::env::set_var(var, SECRET);
    let mut r = rig("env-origin", true);
    // Added over the protocol: the url is not confirmed, the key stays home.
    let mut p = r.profile("envy");
    p["key_ref"] = json!(format!("env:{var}"));
    let o = r.call(json!({"type": "engine_add", "engine": p}));
    assert_eq!(o["engine"]["key_present"], false, "{o}");
    speak_through(&mut r, "envy");
    assert!(
        !saw_a_key(&r.provider),
        "an unconfirmed url never gets the env key"
    );
    let o = r.call(json!({"type": "engine_test", "engine": "envy", "play": false}));
    assert_eq!(o["error"]["reason"], "no_key", "{o}");
    assert!(
        o["error"]["message"]
            .as_str()
            .unwrap()
            .contains("key_origin"),
        "{o}"
    );
    let file = std::fs::read_to_string(r.home.join("engines.json")).unwrap();
    assert!(!file.contains("key_origin"), "{file}");
    // A protocol client cannot confirm it: key_origin in the profile is
    // ignored.
    let mut p = r.profile("envy");
    p["key_ref"] = json!(format!("env:{var}"));
    p["key_origin"] = json!(r.provider.url.trim_end_matches("/v1"));
    r.call(json!({"type": "engine_add", "engine": p, "replace": true}));
    let o = r.call(json!({"type": "engine_test", "engine": "envy", "play": false}));
    assert_eq!(o["error"]["reason"], "no_key", "{o}");
    assert!(!saw_a_key(&r.provider));
    std::env::remove_var(var);
}

#[test]
fn cloud_kinds_never_send_their_key_to_a_new_url_or_region() {
    // ElevenLabs, Google, Gemini and Azure profiles without a url send to the
    // provider's host (Azure's from its region): a replace to a url of
    // another server, over TCP or HTTP, or to another region, deletes the
    // key and never sends it (spec 6.4).
    for (kind, options) in [
        ("elevenlabs", json!({})),
        ("google", json!({})),
        ("gemini", json!({})),
        ("azure", json!({"region": "westeurope"})),
    ] {
        for http in [false, true] {
            let tag = format!("cloud-{kind}-{http}");
            let mut r = rig(&tag, true);
            let p = json!({"id": "cloud", "kind": kind, "voice": "v1", "options": options});
            let o = r.call(json!({"type": "engine_add", "engine": p, "secret": SECRET}));
            assert_eq!(o["engine"]["key_present"], true, "{tag}: {o}");
            let other = Provider::start();
            let mut moved = p.clone();
            moved["url"] = json!(other.url.trim_end_matches("/v1"));
            let req = json!({"type": "engine_add", "engine": moved, "replace": true});
            let o = if http {
                let mut s = Session::http();
                r.server.handle(&mut s, &req).reply
            } else {
                r.call(req)
            };
            assert_eq!(o["ok"], true, "{tag}: {o}");
            assert_eq!(o["engine"]["key_present"], false, "{tag}: {o}");
            assert!(r.keys.get("cloud").unwrap().is_none(), "{tag}");
            speak_through(&mut r, "cloud");
            assert!(!saw_a_key(&other), "{tag}: the key reached the new host");
        }
    }
    let mut r = rig("cloud-region", true);
    let p = json!({"id": "az", "kind": "azure", "voice": "v1",
        "options": {"region": "westeurope"}});
    r.call(json!({"type": "engine_add", "engine": p, "secret": SECRET}));
    let mut moved = p.clone();
    moved["options"]["region"] = json!("eastus");
    let o = r.call(json!({"type": "engine_add", "engine": moved, "replace": true}));
    assert_eq!(o["engine"]["key_present"], false, "{o}");
    assert!(
        r.keys.get("az").unwrap().is_none(),
        "a region change drops the key"
    );
    // A change that keeps the region keeps the key.
    r.call(json!({"type": "engine_add", "engine": p, "secret": SECRET, "replace": true}));
    let mut same = p.clone();
    same["voice"] = json!("v2");
    let o = r.call(json!({"type": "engine_add", "engine": same, "replace": true}));
    assert_eq!(o["engine"]["key_present"], true, "{o}");
}

// ---- command engines are local-only (security review of PR3) -----------

/// A program that exists on every Windows (the profile is only stored and
/// listed here; nothing runs it).
fn real_exe() -> String {
    std::env::var("SystemRoot")
        .map(|r| format!("{r}\\System32\\whoami.exe"))
        .unwrap_or_else(|_| "C:\\Windows\\System32\\whoami.exe".into())
}

fn command_profile(id: &str, program: &str) -> Value {
    json!({"id": id, "kind": "command", "options": {"argv": [program, "--say"]}})
}

/// `engines.json` as the user (or `sonara engines add --kind command`)
/// writes it.
fn write_engines(r: &Rig, engines: Value) {
    std::fs::write(
        r.home.join(engines::FILE),
        json!({"format": 1, "engines": engines}).to_string(),
    )
    .unwrap();
}

fn engines_json(r: &Rig) -> Option<String> {
    std::fs::read_to_string(r.home.join(engines::FILE)).ok()
}

impl Rig {
    /// One request over TCP (the rig's session) or HTTP (a new one).
    fn call_on(&mut self, http: bool, req: Value) -> Value {
        if http {
            self.server.handle(&mut Session::http(), &req).reply
        } else {
            self.call(req)
        }
    }
}

const FORBIDDEN: &str = "a command engine runs a program on this PC, so it is never added or \
     changed over the protocol: add it with `sonara engines add <id> --kind command`, or in \
     engines.json";

#[test]
fn command_engines_are_never_added_over_the_protocol() {
    let mut r = rig("cmd-forbidden", true);
    let missing = format!("{}\\no-such-tts.exe", r.home.display());
    for http in [false, true] {
        for (profile, replace) in [
            (command_profile("prog", &real_exe()), false),
            (command_profile("prog", &real_exe()), true),
            // Refused before any check, so the reply never says whether a
            // path exists on this PC.
            (command_profile("prog", &missing), false),
            (json!({"id": "prog", "kind": "command"}), false),
        ] {
            let o = r.call_on(
                http,
                json!({"type": "engine_add", "engine": profile, "replace": replace}),
            );
            assert_eq!(code(&o), "E_FORBIDDEN", "{o}");
            assert_eq!(o["error"]["message"], FORBIDDEN);
        }
    }
    assert_eq!(engines_json(&r), None, "nothing was saved");
    assert!(r.call(json!({"type": "engine_list"}))["engines"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!r
        .lines
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.starts_with("engine add")));
}

#[test]
fn a_local_command_engine_is_never_replaced_over_the_protocol() {
    let mut r = rig("cmd-replace", true);
    write_engines(&r, json!([command_profile("prog", &real_exe())]));
    let o = r.call(json!({"type": "engine_reload"}));
    assert_eq!(o["ok"], true, "{o}");
    let before = engines_json(&r);
    // Not as another kind either (that would drop the user's program), over
    // TCP and HTTP.
    let mut other = r.profile("prog");
    other["key_ref"] = json!("none");
    for http in [false, true] {
        for p in [other.clone(), command_profile("prog", &real_exe())] {
            let o = r.call_on(
                http,
                json!({"type": "engine_add", "engine": p, "replace": true}),
            );
            assert_eq!(code(&o), "E_FORBIDDEN", "{o}");
        }
    }
    assert_eq!(engines_json(&r), before, "engines.json is unchanged");
    let l = r.call(json!({"type": "engine_list"}));
    assert_eq!(l["engines"][0]["kind"], "command");
    // Selecting it and removing it stay allowed.
    let o = r.call(json!({"type": "set", "key": "engine", "value": "prog"}));
    assert_eq!(o["value"], "prog", "{o}");
    let o = r.call(json!({"type": "engine_remove", "engine": "prog"}));
    assert_eq!(o["removed"], "prog", "{o}");
    assert_eq!(o["engine"], "fake");
    assert!(!engines_json(&r).unwrap().contains("command"));
}

#[test]
fn engine_reload_reads_the_file_the_user_wrote() {
    let mut r = rig("reload", true);
    r.add("local");
    let stored: Value = serde_json::from_str(&engines_json(&r).unwrap()).unwrap();
    let local_entry = stored["engines"][0].clone();
    // The CLI adds a program next to the existing profile.
    write_engines(
        &r,
        json!([local_entry.clone(), command_profile("prog", &real_exe())]),
    );
    let o = r.call(json!({"type": "engine_reload"}));
    assert_eq!(o["ok"], true, "{o}");
    assert_eq!(o["problems"], json!([]));
    let ids: Vec<&str> = o["engines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["local", "prog"]);
    assert_eq!(o["engines"][1]["sends_text_to"], "program whoami.exe");
    assert_eq!(o["engines"][0]["key_present"], true, "keys are untouched");
    let o = r.call(json!({"type": "set", "key": "engine", "value": "prog"}));
    assert_eq!(o["value"], "prog", "registered: {o}");
    // An entry that fails validation is listed with its error.
    let bad = json!({"id": "typo", "kind": "command", "options": {"argv": ["tts.exe"]}});
    write_engines(
        &r,
        json!([local_entry, command_profile("prog", &real_exe()), bad]),
    );
    let o = r.call(json!({"type": "engine_reload"}));
    assert_eq!(o["engines"][2]["id"], "typo");
    assert!(o["engines"][2]["error"]
        .as_str()
        .unwrap()
        .contains("full path"));
    assert_eq!(o["problems"].as_array().unwrap().len(), 1);
    assert_eq!(
        r.call(json!({"type": "get", "key": "engine"}))["value"],
        "prog",
        "an unchanged current engine stays"
    );
    // The current engine removed from the file: the default reads.
    write_engines(&r, json!([]));
    let o = r.call(json!({"type": "engine_reload"}));
    assert_eq!(o["engines"], json!([]));
    assert_eq!(
        r.call(json!({"type": "get", "key": "engine"}))["value"],
        "fake"
    );
    let o = r.call(json!({"type": "set", "key": "engine", "value": "prog"}));
    assert_eq!(code(&o), "E_NOT_FOUND", "unregistered");
    // A file that is not JSON changes nothing.
    r.add("local");
    std::fs::write(r.home.join(engines::FILE), "{not json").unwrap();
    let o = r.call(json!({"type": "engine_reload"}));
    assert_eq!(code(&o), "E_BAD_REQUEST", "{o}");
    assert!(o["error"]["message"]
        .as_str()
        .unwrap()
        .contains("engines.json is not valid JSON"));
    assert_eq!(
        r.call(json!({"type": "engine_list"}))["engines"][0]["id"],
        "local"
    );
}

#[test]
fn engine_reload_takes_no_profile() {
    let mut r = rig("reload-args", true);
    // Whatever else the message carries is ignored: only the file counts.
    let o = r.call(json!({"type": "engine_reload",
        "engine": command_profile("prog", &real_exe()),
        "engines": [command_profile("prog", &real_exe())]}));
    assert_eq!(o["ok"], true, "{o}");
    assert_eq!(o["engines"], json!([]));
    assert_eq!(engines_json(&r), None);
}

/// `engines.json` in the current format, as the user edits it.
fn write_engines_v2(r: &Rig, engines: Value) {
    std::fs::write(
        r.home.join(engines::FILE),
        json!({"format": 2, "engines": engines}).to_string(),
    )
    .unwrap();
}

#[test]
fn a_reload_that_moves_an_address_deletes_the_key() {
    // engine_reload (and a save that first takes in the file's changes) is
    // an edit path too: a key whose origin is no longer its entry's is
    // deleted, never sent to the new address (spec 6.4).
    let mut r = rig("reload-origin", true);
    r.add("local");
    let stored: Value = serde_json::from_str(&engines_json(&r).unwrap()).unwrap();
    let mut entry = stored["engines"][0].clone();
    // Only the voice changes: the key stays.
    entry["voice"] = json!("am_echo");
    write_engines_v2(&r, json!([entry.clone()]));
    let o = r.call(json!({"type": "engine_reload"}));
    assert_eq!(o["engines"][0]["key_present"], true, "{o}");
    assert!(r.keys.get("local").unwrap().is_some());
    // The address moves: the key is deleted and never reaches the new host.
    let other = Provider::start();
    entry["url"] = json!(other.url);
    write_engines_v2(&r, json!([entry.clone()]));
    let o = r.call(json!({"type": "engine_reload"}));
    assert_eq!(o["engines"][0]["key_present"], false, "{o}");
    assert!(r.keys.get("local").unwrap().is_none(), "the key is deleted");
    speak_through(&mut r, "local");
    assert!(!saw_a_key(&other), "the key reached the new host");
    // The same through a save that first reads the changed file.
    let mut r = rig("reload-origin-save", true);
    r.add("local");
    let mut entry = r.profile("local");
    entry["key_ref"] = json!("credman");
    let third = Provider::start();
    entry["url"] = json!(third.url);
    write_engines_v2(&r, json!([entry]));
    let mut extra = r.profile("extra");
    extra["key_ref"] = json!("none");
    let o = r.call(json!({"type": "engine_add", "engine": extra}));
    assert_eq!(o["ok"], true, "{o}");
    assert!(r.keys.get("local").unwrap().is_none(), "the key is deleted");
    speak_through(&mut r, "local");
    assert!(!saw_a_key(&third));
}

#[test]
fn a_command_key_is_bound_to_its_program() {
    let mut r = rig("command-key", true);
    let mut p = command_profile("prog", &real_exe());
    p["key_ref"] = json!("credman");
    write_engines_v2(&r, json!([p.clone()]));
    r.call(json!({"type": "engine_reload"}));
    let o = r.call(json!({"type": "engine_key", "engine": "prog", "secret": SECRET}));
    assert_eq!(o["key_present"], true, "{o}");
    let k = r.keys.get("prog").unwrap().unwrap();
    assert_eq!(
        k.origin.as_deref(),
        Some(format!("command:{}", real_exe().to_lowercase()).as_str())
    );
    // The user points the entry at another program: the key is deleted.
    p["options"]["argv"] = json!([std::env::current_exe().unwrap().display().to_string()]);
    write_engines_v2(&r, json!([p]));
    let o = r.call(json!({"type": "engine_reload"}));
    assert_eq!(o["engines"][0]["key_present"], false, "{o}");
    assert!(r.keys.get("prog").unwrap().is_none());
}

#[test]
fn voices_of_an_unsaved_profile_use_the_key_sent_and_save_nothing() {
    // Protocol 1.4 (#227): the settings page lists a new engine's voices
    // before it is saved, so a voice need not be typed up front.
    let mut r = rig("draft", true);
    let p = r.profile("not-saved");
    let o = r.call(json!({"type": "voices", "profile": p, "secret": SECRET}));
    assert_eq!(o["ok"], true, "{o}");
    let ids: Vec<&str> = o["voices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["af_heart", "am_echo"]);
    assert!(saw_a_key(&r.provider), "the key sent went to the server");
    assert!(!o.to_string().contains(SECRET));
    // Nothing is saved: no profile, no key, no file, no registered engine.
    assert!(r.call(json!({"type": "engine_list"}))["engines"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(r.keys.list().unwrap().is_empty());
    assert_eq!(engines_json(&r), None);
    let all = r.call(json!({"type": "voices"}));
    assert!(!all.to_string().contains("am_echo"), "{all}");
    for l in r.lines.lock().unwrap().iter() {
        assert!(!l.contains(SECRET), "a log line: {l}");
    }
}

#[test]
fn an_unsaved_cloud_profile_lists_voices_without_a_voice_yet() {
    let mut r = rig("draft-cloud", true);
    let root = r.provider.url.trim_end_matches("/v1").to_string();
    let o = r.call(json!({"type": "voices", "secret": SECRET,
        "profile": {"kind": "elevenlabs", "url": root}}));
    // The fake provider is no ElevenLabs: the list fails, but not for a
    // missing voice or id, and no placeholder leaks into the list.
    assert_eq!(o["ok"], true, "{o}");
    assert!(o["voices"].as_array().unwrap().is_empty(), "{o}");
    assert!(o["error"]["message"].is_string(), "{o}");
    assert!(
        r.provider
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|(p, _, _)| p.starts_with("/v2/voices")),
        "the voice list was asked for"
    );
}

#[test]
fn an_unsaved_profile_is_never_a_program_and_needs_engines() {
    let mut r = rig("draft-cmd", true);
    let o = r.call(json!({"type": "voices",
        "profile": command_profile("prog", &real_exe())}));
    assert_eq!(code(&o), "E_FORBIDDEN", "{o}");
    let o = r.call(json!({"type": "voices", "profile": "x"}));
    assert_eq!(code(&o), "E_BAD_REQUEST", "{o}");
    let mut off = rig("draft-off", false);
    let p = off.profile("x");
    let o = off.call(json!({"type": "voices", "profile": p}));
    assert_eq!(code(&o), "E_UNSUPPORTED", "{o}");
}
