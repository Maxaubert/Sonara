//! The `system` extension in `sonarad` on the fake platform: the keys,
//! arming per client (restore_on_client_drop), keep_alive, hotkey dispatch
//! to core, channel and agent controls, and the restore at exit.
use serde_json::{json, Value};
use sonara_audio::TestOutput;
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config, ReaderHandle, Registry};
use sonara_system::fake::Fake;
use sonara_system::keymap::Action;
use sonarad::config::{self, Store};
use sonarad::lifetime::Lifetime;
use sonarad::protocol::{Server, Session};
use sonarad::system_ext::SystemHost;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

const TOKEN: &str = "secret";

/// A fresh home in `%TEMP%`, removed when the test ends.
struct Tmp(PathBuf);

impl std::ops::Deref for Tmp {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tmp() -> Tmp {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "sonarad-system-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Tmp(dir)
}

struct Rig {
    server: Arc<Server>,
    fake: Fake,
    out: TestOutput,
    home: PathBuf,
    /// Last, so the server is gone before its home is removed.
    _tmp: Option<Tmp>,
}

fn fake_registry() -> Registry {
    let registry = Registry::default();
    registry.register(Arc::new(FakeEngine::new())).unwrap();
    registry
}

fn rig() -> Rig {
    let t = tmp();
    let mut r = rig_on(t.to_path_buf());
    r._tmp = Some(t);
    r
}

/// A rig on `home`, with the settings persisted there.
fn rig_on(home: PathBuf) -> Rig {
    let registry = Registry::default();
    registry.register(Arc::new(FakeEngine::new())).unwrap();
    let (out, rx) = TestOutput::new();
    let reader =
        ReaderHandle::new(Config::new(registry).with_output(Box::new(out.clone()), rx)).unwrap();
    let fake = Fake::new();
    fake.add_audio(100, "vlc.exe", 0.8);
    let (store, problems) = Store::load(&home);
    assert!(problems.is_empty(), "{problems:?}");
    let server = Server::new(
        reader,
        TOKEN.into(),
        Lifetime::new(Duration::from_secs(30), false),
    )
    .with_config(store)
    .with_log(log_to(&home))
    .with_earcons(earcons_of(&home))
    .with_system(SystemHost {
        platform: fake.platform(),
        home: home.clone(),
        http_port: 4321,
        token: TOKEN.into(),
        previews: Some(Arc::new(fake_registry())),
    });
    Rig {
        server: Arc::new(server),
        fake,
        out,
        home,
        _tmp: None,
    }
}

/// The activity lines, to the `sonarad.log` of `home` (as `main`).
fn log_to(home: &std::path::Path) -> sonara_system::LogFn {
    let h = sonarad::home::Home {
        dir: home.to_path_buf(),
        is_default: false,
    };
    Arc::new(move |line: &str| h.log(line))
}

fn log_of(r: &Rig) -> String {
    std::fs::read_to_string(r.home.join("logs").join("sonarad.log")).unwrap_or_default()
}

/// The custom earcons of `home`, logged to its `sonarad.log` (as `main`).
fn earcons_of(home: &std::path::Path) -> Arc<sonara_agent::Library> {
    let h = sonarad::home::Home {
        dir: home.to_path_buf(),
        is_default: false,
    };
    Arc::new(sonara_agent::Library::new(
        h.earcons(),
        Some(Arc::new(move |line: &str| h.log(line))),
    ))
}

fn call(s: &Server, session: &mut Session, req: Value) -> Value {
    s.handle(session, &req).reply
}

fn ok(s: &Server, session: &mut Session, req: Value) -> Value {
    let r = call(s, session, req.clone());
    assert_eq!(r["ok"], true, "{req} -> {r}");
    r
}

fn eventually(mut pred: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    pred()
}

/// Bind the rate hotkeys (unbound by default) through the protocol.
fn bind_rate_keys(s: &Server) {
    let mut h = Session::http();
    for (action, key) in [("faster", "]"), ("slower", "[")] {
        ok(
            s,
            &mut h,
            json!({"type": "set", "key": "hotkeys",
                   "value": {"action": action, "key": key, "mods": ["ctrl", "alt"]}}),
        );
    }
}

/// Let the output start the chunk the reader handed it.
fn start_playing(out: &TestOutput) {
    assert!(eventually(|| out.loaded().is_some()), "nothing loaded");
    out.start();
}

fn ducked(fake: &Fake) -> bool {
    (fake.volume(100).unwrap() - 0.25).abs() < 1e-4
}

fn restored(fake: &Fake) -> bool {
    (fake.volume(100).unwrap() - 0.8).abs() < 1e-4
}

#[test]
fn the_extension_is_offered_and_its_keys_answer_once_enabled() {
    let r = rig();
    let s = &r.server;
    assert!(s.offered().contains(&"system"));
    let mut session = Session::http();
    let e = call(s, &mut session, json!({"type": "get", "key": "audio_mode"}));
    assert_eq!(e["error"]["code"], "E_UNSUPPORTED", "not enabled yet");
    let h = ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system"]}),
    );
    assert!(h["extensions"]
        .as_array()
        .unwrap()
        .contains(&json!("system")));
    let g = ok(s, &mut session, json!({"type": "get", "key": "audio_mode"}));
    assert_eq!(g["value"], "pause", "the product default (#202)");
    let g = ok(
        s,
        &mut session,
        json!({"type": "set", "key": "audio_mode", "value": "duck"}),
    );
    assert_eq!(g["value"], "duck");
    let e = call(
        s,
        &mut session,
        json!({"type": "set", "key": "audio_mode", "value": "loud"}),
    );
    assert_eq!(e["error"]["code"], "E_BAD_REQUEST");
    let g = ok(s, &mut session, json!({"type": "get", "key": "duck_level"}));
    assert_eq!(g["value"], 30);
    let e = call(
        s,
        &mut session,
        json!({"type": "set", "key": "duck_level", "value": 101}),
    );
    assert_eq!(e["error"]["code"], "E_BAD_REQUEST");
    let g = ok(
        s,
        &mut session,
        json!({"type": "get", "key": "settings_url"}),
    );
    assert_eq!(g["value"], "http://127.0.0.1:4321/settings?token=secret");
    let e = call(
        s,
        &mut session,
        json!({"type": "set", "key": "settings_url", "value": "x"}),
    );
    assert_eq!(e["error"]["code"], "E_BAD_REQUEST");
}

#[test]
fn hotkeys_bind_unbind_and_reset_through_set() {
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system"]}),
    );
    let g = ok(s, &mut session, json!({"type": "get", "key": "hotkeys"}));
    let v = &g["value"];
    assert_eq!(v["active"], false, "an HTTP hello does not arm the hotkeys");
    let restart = &v["bindings"][0];
    assert_eq!(restart["action"], "restart");
    assert_eq!(restart["combo"], "Ctrl+Alt+Up");
    let g = ok(
        s,
        &mut session,
        json!({"type": "set", "key": "hotkeys", "value": {"action": "mute", "key": "k", "mods": ["win", "alt"]}}),
    );
    let mute = g["value"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["action"] == "mute")
        .unwrap()
        .clone();
    assert_eq!(mute["combo"], "Win+Alt+K");
    let on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(r.home.join("keymap.json")).unwrap())
            .unwrap();
    assert_eq!(on_disk["mute"], json!({"key": "k", "mods": ["win", "alt"]}));
    for bad in [
        json!({"action": "mute", "key": "m", "mods": ["shift"]}),
        json!({"action": "mute", "key": "escape", "mods": ["ctrl"]}),
        json!({"action": "warp", "key": "m", "mods": ["ctrl"]}),
        json!(42),
    ] {
        let e = call(
            s,
            &mut session,
            json!({"type": "set", "key": "hotkeys", "value": bad}),
        );
        assert_eq!(e["error"]["code"], "E_BAD_REQUEST", "{bad}");
    }
    let g = ok(
        s,
        &mut session,
        json!({"type": "set", "key": "hotkeys", "value": {"action": "restart", "key": null}}),
    );
    assert_eq!(g["value"]["bindings"][0]["key"], Value::Null);
    let g = ok(
        s,
        &mut session,
        json!({"type": "set", "key": "hotkeys", "value": "reset"}),
    );
    assert_eq!(g["value"]["bindings"][0]["key"], "up");
    assert_eq!(g["value"]["bindings"][3]["key"], "m");
}

async fn tcp_client(port: u16, hello: Value) -> (BufReader<TcpStream>, Value) {
    let mut c = BufReader::new(TcpStream::connect(("127.0.0.1", port)).await.unwrap());
    let reply = request(&mut c, hello).await;
    (c, reply)
}

async fn request(c: &mut BufReader<TcpStream>, msg: Value) -> Value {
    let mut line = msg.to_string();
    line.push('\n');
    c.get_mut().write_all(line.as_bytes()).await.unwrap();
    loop {
        let mut buf = String::new();
        c.read_line(&mut buf).await.unwrap();
        let v: Value = serde_json::from_str(&buf).unwrap();
        if v.get("ok").is_some() {
            return v;
        }
    }
}

async fn serve(r: &Rig) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(sonarad::tcp::serve(listener, r.server.clone()));
    port
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_on_client_drop() {
    // The client that enabled `system` goes away mid-utterance: other apps
    // are restored at once although the reader keeps reading.
    let r = rig();
    let port = serve(&r).await;
    let (mut c, h) = tcp_client(
        port,
        json!({"type": "hello", "token": TOKEN, "extensions": ["system"]}),
    )
    .await;
    assert_eq!(h["ok"], true);
    request(
        &mut c,
        json!({"type": "set", "key": "audio_mode", "value": "duck"}),
    )
    .await;
    request(
        &mut c,
        json!({"type": "set", "key": "duck_level", "value": 25}),
    )
    .await;
    request(
        &mut c,
        json!({"type": "speak", "text": "One long sentence."}),
    )
    .await;
    let fake = r.fake.clone();
    let out = r.out.clone();
    assert!(
        tokio::task::spawn_blocking(move || {
            start_playing(&out);
            eventually(|| ducked(&fake))
        })
        .await
        .unwrap(),
        "ducked while speaking"
    );
    assert!(r.home.join("state").join("duck_state.json").exists());
    drop(c);
    let fake = r.fake.clone();
    assert!(
        tokio::task::spawn_blocking(move || eventually(|| restored(&fake)))
            .await
            .unwrap(),
        "restored when the client left"
    );
    assert!(r.server.reader().state().unwrap().now_playing.is_some());
    assert!(!r.home.join("state").join("duck_state.json").exists());
    assert!(r.fake.world().hotkeys.is_empty(), "hotkeys released");
}

#[tokio::test(flavor = "multi_thread")]
async fn another_client_that_needs_it_keeps_it_armed() {
    let r = rig();
    let port = serve(&r).await;
    let hello = json!({"type": "hello", "token": TOKEN, "extensions": ["system"]});
    let (mut a, _) = tcp_client(port, hello.clone()).await;
    let (b, _) = tcp_client(port, hello).await;
    request(
        &mut a,
        json!({"type": "set", "key": "audio_mode", "value": "duck"}),
    )
    .await;
    request(
        &mut a,
        json!({"type": "set", "key": "duck_level", "value": 25}),
    )
    .await;
    request(&mut a, json!({"type": "speak", "text": "Long."})).await;
    let (fake, out) = (r.fake.clone(), r.out.clone());
    assert!(tokio::task::spawn_blocking(move || {
        start_playing(&out);
        eventually(|| ducked(&fake))
    })
    .await
    .unwrap());
    drop(b);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(ducked(&r.fake), "client a still needs it");
    assert_eq!(r.fake.world().hotkeys.len(), 4);
    drop(a);
    let fake = r.fake.clone();
    assert!(
        tokio::task::spawn_blocking(move || eventually(|| restored(&fake)))
            .await
            .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_alive_arms_for_good_and_exit_restores() {
    let r = rig();
    let mut session = Session::http();
    ok(
        &r.server,
        &mut session,
        json!({"type": "hello", "extensions": ["system"], "keep_alive": true}),
    );
    ok(
        &r.server,
        &mut session,
        json!({"type": "set", "key": "audio_mode", "value": "duck"}),
    );
    ok(
        &r.server,
        &mut session,
        json!({"type": "set", "key": "duck_level", "value": 25}),
    );
    ok(
        &r.server,
        &mut session,
        json!({"type": "speak", "text": "Long."}),
    );
    start_playing(&r.out);
    assert!(eventually(|| ducked(&r.fake)));
    assert_eq!(r.fake.world().hotkeys.len(), 4);
    // The runtime exits (as main does after its run loop).
    let s = r.server.system().unwrap().clone();
    tokio::task::spawn_blocking(move || s.shutdown())
        .await
        .unwrap();
    assert!(restored(&r.fake));
    assert!(r.fake.world().hotkeys.is_empty());
}

#[test]
fn a_hotkey_press_runs_the_matching_control() {
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system"], "keep_alive": true}),
    );
    bind_rate_keys(s);
    let reader = s.reader().clone();
    r.fake.press(Action::Faster.id());
    assert!(eventually(|| reader.state().unwrap().rate == 225));
    r.fake.press(Action::Slower.id());
    r.fake.press(Action::Slower.id());
    assert!(eventually(|| reader.state().unwrap().rate == 175));
    // Without the agent extension, mute toggles the core mute.
    r.fake.press(Action::Mute.id());
    assert!(eventually(|| reader.state().unwrap().muted));
    std::thread::sleep(sonara_system::hotkeys::DEBOUNCE + Duration::from_millis(50));
    r.fake.press(Action::Mute.id());
    assert!(eventually(|| !reader.state().unwrap().muted));
    ok(
        s,
        &mut session,
        json!({"type": "speak", "text": "One. Two."}),
    );
    start_playing(&r.out);
    r.fake.press(Action::Flush.id());
    assert!(eventually(|| reader.state().unwrap().now_playing.is_none()));
}

#[test]
fn hotkeys_drive_the_agent_mute_cycle_and_channel_switches() {
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system", "agent"], "keep_alive": true}),
    );
    let level = |s: &Server| {
        let mut h = Session::http();
        s.handle(&mut h, &json!({"type": "get", "key": "mute_level"}))
            .reply["value"]
            .clone()
    };
    r.fake.press(Action::Mute.id());
    assert!(eventually(|| level(s) == json!(1)));
    std::thread::sleep(sonara_system::hotkeys::DEBOUNCE + Duration::from_millis(50));
    r.fake.press(Action::Mute.id());
    assert!(eventually(|| level(s) == json!(2)));
    std::thread::sleep(sonara_system::hotkeys::DEBOUNCE + Duration::from_millis(50));
    r.fake.press(Action::Mute.id());
    assert!(eventually(|| level(s) == json!(0)));
    ok(
        s,
        &mut session,
        json!({"type": "channel_open", "channel": "a"}),
    );
    ok(
        s,
        &mut session,
        json!({"type": "channel_open", "channel": "b"}),
    );
    ok(
        s,
        &mut session,
        json!({"type": "speak", "channel": "a", "text": "Alpha."}),
    );
    ok(
        s,
        &mut session,
        json!({"type": "speak", "channel": "b", "text": "Beta."}),
    );
    let channels = s.channels().unwrap().clone();
    assert!(eventually(|| channels.engaged().as_deref() == Some("a")));
    r.fake.press(Action::NextChannel.id());
    assert!(eventually(|| channels.engaged().as_deref() == Some("b")));
    // One support log line per action, saying what it did (#217).
    assert!(eventually(
        || log_of(&r).contains("hotkey next_channel session=b")
    ));
    let log = log_of(&r);
    let at = |l: &str| log.find(l).unwrap_or_else(|| panic!("no '{l}' in {log}"));
    assert!(at("hotkey mute level=1") < at("hotkey mute level=2"));
    assert!(at("hotkey mute level=2") < at("hotkey mute level=0"));
}

#[test]
fn an_ask_is_logged_with_its_kind_and_session_and_no_text_with_debug_log_off() {
    // No other test of this binary needs the text in the log (#219).
    sonarad::trace_log::set_debug(false);
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["agent"]}),
    );
    ok(
        s,
        &mut session,
        json!({"type": "channel_open", "channel": "a", "label": "Work"}),
    );
    ok(
        s,
        &mut session,
        json!({"type": "ask", "channel": "a", "kind": "permission", "text": "Run secret command"}),
    );
    let log = log_of(&r);
    assert!(log.contains("ask kind=permission session=Work"), "{log}");
    assert!(!log.contains("secret"), "{log}");
}

/// Open labelled channels `a` and `b` (as the Claude hook does, with the
/// session's folder) and give each a message; wait until `a` reads.
fn two_sessions(r: &Rig, session: &mut Session) {
    let s = &r.server;
    for (id, label) in [("a", "alpha-repo"), ("b", "beta-repo")] {
        ok(
            s,
            session,
            json!({"type": "channel_open", "channel": id, "label": label}),
        );
    }
    ok(
        s,
        session,
        json!({"type": "speak", "channel": "a", "text": "Alpha words."}),
    );
    ok(
        s,
        session,
        json!({"type": "speak", "channel": "b", "text": "Beta words."}),
    );
    let reader = s.reader().clone();
    assert!(eventually(|| reader
        .state()
        .unwrap()
        .now_playing
        .is_some_and(|n| n.text == "Alpha words.")));
}

fn playing(s: &Server) -> Option<String> {
    s.reader().state().unwrap().now_playing.map(|n| n.text)
}

/// The last `PlayClip` comes before the last `Play` (the earcon was handed
/// to the output before the announcement).
fn clip_before_last_play(out: &TestOutput) -> bool {
    let calls = out.calls();
    let clip = calls
        .iter()
        .rposition(|c| matches!(c, sonara_audio::OutputCall::PlayClip { .. }));
    let play = calls
        .iter()
        .rposition(|c| matches!(c, sonara_audio::OutputCall::Play { .. }));
    matches!((clip, play), (Some(c), Some(p)) if c < p)
}

#[test]
fn the_next_session_hotkey_chimes_then_says_session_changed() {
    // Python controls.py on_next_session + "Session changed: {0}." (#209).
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system", "agent"], "keep_alive": true}),
    );
    let chimes = s.agent().unwrap().subscribe();
    two_sessions(&r, &mut session);
    r.fake.press(Action::NextChannel.id());
    assert!(
        eventually(|| playing(s).as_deref() == Some("Session changed: beta-repo.")),
        "playing {:?}",
        playing(s)
    );
    assert!(eventually(|| r.out.loaded().is_some()));
    assert!(clip_before_last_play(&r.out), "{:?}", r.out.calls());
    assert_eq!(
        chimes.recv_timeout(Duration::from_secs(5)).unwrap(),
        sonara_agent::Earcon::SessionChange
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(chimes.try_recv().is_err(), "one chime per press");
    // Announcements off: the press still chimes, nothing is said.
    ok(
        s,
        &mut session,
        json!({"type": "set", "key": "channel_announce", "value": "off"}),
    );
    settle_debounce();
    r.fake.press(Action::NextChannel.id());
    assert_eq!(
        chimes.recv_timeout(Duration::from_secs(5)).unwrap(),
        sonara_agent::Earcon::SessionChange
    );
    assert!(eventually(
        || playing(s).is_some_and(|t| !t.starts_with("Session changed"))
    ));
}

#[test]
fn the_next_session_hotkey_on_a_session_without_a_label_chimes_once() {
    // #241: an unnamed session is announced "Session changed." with the
    // agent's chime, so the hotkey must not chime a second time.
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system", "agent"], "keep_alive": true}),
    );
    let chimes = s.agent().unwrap().subscribe();
    ok(
        s,
        &mut session,
        json!({"type": "channel_open", "channel": "a", "label": "alpha-repo"}),
    );
    for (id, text) in [("a", "Alpha words."), ("b", "Beta words.")] {
        ok(
            s,
            &mut session,
            json!({"type": "speak", "channel": id, "text": text}),
        );
    }
    assert!(eventually(|| playing(s).as_deref() == Some("Alpha words.")));
    r.fake.press(Action::NextChannel.id());
    assert!(
        eventually(|| playing(s).as_deref() == Some("Session changed.")),
        "playing {:?}",
        playing(s)
    );
    assert_eq!(
        chimes.recv_timeout(Duration::from_secs(5)).unwrap(),
        sonara_agent::Earcon::SessionChange
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(chimes.try_recv().is_err(), "one chime per press");
}

#[test]
fn an_automatic_session_switch_chimes_then_says_session_changed() {
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system", "agent"], "keep_alive": true}),
    );
    let chimes = s.agent().unwrap().subscribe();
    two_sessions(&r, &mut session);
    assert!(chimes.try_recv().is_err());
    // The item is now playing before its chunk reaches the output: wait
    // for the load, or `start` finds nothing loaded (a flaky panic).
    assert!(eventually(|| r.out.loaded().is_some()));
    r.out.start();
    r.out.finish();
    assert!(eventually(
        || playing(s).as_deref() == Some("Session changed: beta-repo.")
    ));
    assert_eq!(
        chimes.recv_timeout(Duration::from_secs(5)).unwrap(),
        sonara_agent::Earcon::SessionChange
    );
    assert!(eventually(|| r.out.loaded().is_some()));
    assert!(clip_before_last_play(&r.out), "{:?}", r.out.calls());
}

#[test]
fn custom_earcons_are_listed_played_and_bad_files_logged() {
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["agent"]}),
    );
    let v = ok(s, &mut session, json!({"type": "get", "key": "earcons"}))["value"].clone();
    assert_eq!(v["custom"], json!([]));
    assert_eq!(v["kinds"].as_array().unwrap().len(), 8);
    let folder = r.home.join("earcons");
    assert_eq!(v["folder"], json!(folder.display().to_string()));
    std::fs::create_dir_all(&folder).unwrap();
    let pcm = sonara_engine::PcmChunk {
        samples: (0..800)
            .map(|i| if i % 16 < 8 { 8000 } else { -8000 })
            .collect(),
        sample_rate: 8000,
        channels: 1,
    };
    std::fs::write(
        folder.join("turn_done.wav"),
        sonara_engine::wav::encode(&pcm),
    )
    .unwrap();
    std::fs::write(folder.join("choice.wav"), b"not a wav").unwrap();
    let v = ok(s, &mut session, json!({"type": "get", "key": "earcons"}))["value"].clone();
    assert_eq!(v["custom"], json!(["turn_done"]));
    let r2 = call(
        s,
        &mut session,
        json!({"type": "set", "key": "earcons", "value": {}}),
    );
    assert_eq!(r2["error"]["code"], "E_BAD_REQUEST", "{r2}");
    ok(
        s,
        &mut session,
        json!({"type": "earcon", "kind": "turn_done"}),
    );
    assert!(eventually(|| r.out.calls().contains(
        &sonara_audio::OutputCall::PlayClip {
            samples: 800,
            sample_rate: 8000
        }
    )));
    let log = std::fs::read_to_string(r.home.join("logs").join("sonarad.log")).unwrap();
    assert!(
        log.contains("choice.wav") && log.contains("using the bundled choice clip"),
        "{log}"
    );
}

#[test]
fn hotkeys_do_nothing_after_a_takeover() {
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system"], "keep_alive": true}),
    );
    bind_rate_keys(s);
    let mut other = Session::tcp();
    ok(
        s,
        &mut other,
        json!({"type": "hello", "token": TOKEN, "takeover": true}),
    );
    r.fake.press(Action::Faster.id());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(s.reader().state().unwrap().rate, 200);
}

#[test]
fn the_startup_sweep_restores_what_a_killed_runtime_left() {
    let r = rig();
    let state = r.home.join("state");
    std::fs::create_dir_all(&state).unwrap();
    r.fake.edit(|w| w.audio[0].volume = 0.25);
    std::fs::write(
        state.join("duck_state.json"),
        json!({"sessions": [{"pid": 100, "name": "vlc.exe", "original": 0.8}]}).to_string(),
    )
    .unwrap();
    r.server.system().unwrap().recover();
    assert!(restored(&r.fake));
    assert!(!state.join("duck_state.json").exists());
}

fn saved(home: &std::path::Path) -> Value {
    std::fs::read_to_string(home.join("config.json"))
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap_or(Value::Null)
}

#[test]
fn the_schema_defaults_are_the_layers_defaults() {
    let r = rig();
    let s = &r.server;
    // As `main` does before serving: the reader takes the L1 defaults.
    let (store, _) = Store::load(&r.home);
    assert!(config::apply_reader(&store, s.reader(), false).is_empty());
    let mut h = Session::http();
    ok(
        s,
        &mut h,
        json!({"type": "hello", "extensions": ["system", "agent"]}),
    );
    for key in [
        "rate",
        "volume",
        "channel_announce",
        "mute_level",
        "verbosity",
        "read_mode",
        "flush_scope",
        "minqueue",
        "background_policy",
        "audio_mode",
        "duck_level",
    ] {
        let g = ok(s, &mut h, json!({"type": "get", "key": key}));
        assert_eq!(Some(g["value"].clone()), config::default(key), "{key}");
    }
    let g = ok(s, &mut h, json!({"type": "get", "key": "summaries"}));
    let d = config::default("summaries").unwrap();
    for (field, v) in d.as_object().unwrap() {
        assert_eq!(&g["value"][field], v, "summaries.{field}");
    }
    assert_eq!(saved(&r.home), Value::Null, "reading writes nothing");
}

#[test]
fn hotkey_changes_are_persisted_like_a_set() {
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system", "agent"], "keep_alive": true}),
    );
    bind_rate_keys(s);
    r.fake.press(Action::Faster.id());
    assert!(eventually(|| saved(&r.home)["rate"] == 225));
    r.fake.press(Action::Mute.id());
    assert!(eventually(|| saved(&r.home)["mute_level"] == 1));
}

#[test]
fn persisted_settings_apply_when_each_layer_starts() {
    let home = tmp();
    std::fs::write(
        home.join("config.json"),
        r#"{"audio_mode": "duck", "duck_level": 25, "mute_level": 2, "verbosity": "quiet",
            "channel_announce": "off", "summaries": {"style": "brief", "prompts": {"brief": "Short."}}}"#,
    )
    .unwrap();
    let r = rig_on(home.to_path_buf());
    let s = &r.server;
    let mut h = Session::http();
    ok(
        s,
        &mut h,
        json!({"type": "hello", "extensions": ["system", "agent"]}),
    );
    let get = |key: &str| {
        let mut h = Session::http();
        s.handle(&mut h, &json!({"type": "get", "key": key})).reply["value"].clone()
    };
    assert_eq!(get("audio_mode"), "duck");
    assert_eq!(get("duck_level"), 25);
    assert_eq!(get("mute_level"), 2);
    assert_eq!(
        get("verbosity"),
        "skip_code",
        "#214: quiet loads as skip_code"
    );
    assert_eq!(get("channel_announce"), "off");
    assert_eq!(get("summaries")["style"], "brief");
    assert_eq!(get("summaries")["prompt"], "Short.");
}

#[test]
fn a_set_persists_the_value_in_force_and_only_that_key() {
    let r = rig();
    let s = &r.server;
    let mut h = Session::http();
    ok(
        s,
        &mut h,
        json!({"type": "hello", "extensions": ["system", "agent"]}),
    );
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "voice", "value": "Fake silence"}),
    );
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "duck_level", "value": 40}),
    );
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "summaries", "value": {"enabled": false, "model": "sonnet"}}),
    );
    let e = call(
        s,
        &mut h,
        json!({"type": "set", "key": "rate", "value": 999}),
    );
    assert_eq!(e["error"]["code"], "E_BAD_REQUEST");
    assert_eq!(
        saved(&r.home),
        json!({"voice": "silence", "duck_level": 40,
               "summaries": {"enabled": false, "model": "sonnet"}})
    );
}

#[test]
fn channel_prefs_are_stored_and_the_label_replaces_the_clients() {
    let r = rig();
    let s = &r.server;
    let mut h = Session::http();
    ok(
        s,
        &mut h,
        json!({"type": "hello", "extensions": ["system", "agent"]}),
    );
    ok(
        s,
        &mut h,
        json!({"type": "channel_open", "channel": "s1", "label": "repo"}),
    );
    let g = ok(
        s,
        &mut h,
        json!({"type": "set", "key": "channel_prefs",
               "value": {"channel": "s1", "label": "Build", "voice": "silence", "muted": true}}),
    );
    let row = &g["value"][0];
    assert_eq!(row["channel"], "s1");
    assert_eq!(row["client_label"], "repo");
    assert_eq!(row["label"], "Build");
    assert_eq!(row["muted"], true);
    let channels = s.channels().unwrap().clone();
    assert_eq!(
        channels.channel("s1").unwrap().label.as_deref(),
        Some("Build")
    );
    // The client's next channel_open keeps the user's name.
    ok(
        s,
        &mut h,
        json!({"type": "channel_open", "channel": "s1", "label": "repo"}),
    );
    assert_eq!(
        channels.channel("s1").unwrap().label.as_deref(),
        Some("Build")
    );
    // A channel opened by its first text gets the user's name too.
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "channel_prefs", "value": {"channel": "s2", "label": "Docs"}}),
    );
    ok(
        s,
        &mut h,
        json!({"type": "stream", "channel": "s2", "delta": "Hi.", "final": true}),
    );
    assert_eq!(
        channels.channel("s2").unwrap().label.as_deref(),
        Some("Docs")
    );
    let prefs: Value =
        serde_json::from_str(&std::fs::read_to_string(r.home.join("session_prefs.json")).unwrap())
            .unwrap();
    assert_eq!(prefs["s1"]["label"], "Build");
    assert_eq!(prefs["s1"]["voice"], "silence");
    assert_eq!(prefs["s2"]["label"], "Docs");
    // Clearing the label gives the client's back.
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "channel_prefs", "value": {"channel": "s1", "label": null}}),
    );
    assert_eq!(
        channels.channel("s1").unwrap().label.as_deref(),
        Some("repo")
    );
    for bad in [
        json!({"label": "x"}),
        json!({"channel": "s1", "muted": "yes"}),
        json!({"channel": "s1", "label": 3}),
        json!("s1"),
    ] {
        let e = call(
            s,
            &mut h,
            json!({"type": "set", "key": "channel_prefs", "value": bad}),
        );
        assert_eq!(e["error"]["code"], "E_BAD_REQUEST", "{bad}");
    }
}

#[test]
fn a_preview_plays_a_clip_and_leaves_the_queue_alone() {
    let r = rig();
    let s = &r.server;
    let mut h = Session::http();
    let e = call(s, &mut h, json!({"type": "preview"}));
    assert_eq!(e["error"]["code"], "E_UNSUPPORTED", "system not enabled");
    ok(
        s,
        &mut h,
        json!({"type": "hello", "extensions": ["system"]}),
    );
    ok(s, &mut h, json!({"type": "speak", "text": "One. Two."}));
    start_playing(&r.out);
    let before = s.reader().state().unwrap();
    let _ = r.out.take_calls();
    let p = ok(
        s,
        &mut h,
        json!({"type": "preview", "voice": "Fake silence"}),
    );
    assert_eq!(p["voice"], "silence");
    assert_eq!(p["engine"], "fake");
    let calls = r.out.take_calls();
    assert!(
        matches!(calls.as_slice(), [sonara_audio::OutputCall::PlayClip { samples, .. }] if *samples > 0),
        "{calls:?}"
    );
    let after = s.reader().state().unwrap();
    assert_eq!(after.now_playing, before.now_playing);
    assert_eq!(after.queued, before.queued);
    let e = call(s, &mut h, json!({"type": "preview", "voice": "nobody"}));
    assert_eq!(e["error"]["code"], "E_NOT_FOUND");
    let g = ok(s, &mut h, json!({"type": "get", "key": "runtime"}));
    assert_eq!(g["value"]["pid"], std::process::id());
    assert_eq!(g["value"]["previews"], true);
    // #214: the settings page shows the engine's readiness from here.
    assert_eq!(g["value"]["engine_status"]["engine"], "fake");
    assert_eq!(g["value"]["engine_status"]["ready"], true);
}

// -- spoken control cues, per-channel mute, background policy (#195-#197) --

/// The next cues spoken, as texts (waits for `n`).
fn cues_heard(stream: &sonarad::cues::CueStream, n: usize) -> Vec<String> {
    let mut got = Vec::new();
    let end = Instant::now() + Duration::from_secs(10);
    while got.len() < n && Instant::now() < end {
        if let Ok(t) = stream.recv_timeout(Duration::from_millis(50)) {
            got.push(t);
        }
    }
    got
}

fn settle_debounce() {
    std::thread::sleep(sonara_system::hotkeys::DEBOUNCE + Duration::from_millis(50));
}

fn is_clip(c: &sonara_audio::OutputCall) -> bool {
    matches!(c, sonara_audio::OutputCall::PlayClip { samples, .. } if *samples > 0)
}

#[test]
fn hotkeys_speak_their_cues_over_a_paused_reader() {
    // Python controls.py / settings.py: "Paused.", "Resumed.", the mute
    // cycle and "Rate N." are spoken (mute and pause exempt).
    let r = rig();
    let s = &r.server;
    let mut h = Session::http();
    ok(
        s,
        &mut h,
        json!({"type": "hello", "extensions": ["system", "agent"], "keep_alive": true}),
    );
    bind_rate_keys(s);
    // Pause ships unbound.
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "hotkeys",
               "value": {"action": "pause", "key": "k", "mods": ["ctrl", "alt"]}}),
    );
    let cues = s.system().unwrap().cues().subscribe();
    ok(s, &mut h, json!({"type": "speak", "text": "One. Two."}));
    start_playing(&r.out);
    let before = r.out.calls().len();
    r.fake.press(Action::Pause.id());
    assert_eq!(cues_heard(&cues, 1), ["Paused."]);
    assert!(s.reader().state().unwrap().paused);
    assert!(
        r.out.calls()[before..].iter().any(is_clip),
        "the cue is played as a clip while paused"
    );
    settle_debounce();
    r.fake.press(Action::Pause.id());
    assert_eq!(cues_heard(&cues, 1), ["Resumed."]);
    for want in ["Muted.", "Super muted.", "Unmuted."] {
        settle_debounce();
        r.fake.press(Action::Mute.id());
        assert_eq!(cues_heard(&cues, 1), [want]);
    }
    r.fake.press(Action::Faster.id());
    assert_eq!(cues_heard(&cues, 1), ["Rate 225."]);
    r.fake.press(Action::NextChannel.id());
    assert_eq!(cues_heard(&cues, 1), ["No session."]);
}

#[test]
fn setting_changes_speak_their_cues() {
    let r = rig();
    let s = &r.server;
    let mut h = Session::http();
    ok(
        s,
        &mut h,
        json!({"type": "hello", "extensions": ["system", "agent"]}),
    );
    let cues = s.system().unwrap().cues().subscribe();
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "audio_mode", "value": "duck"}),
    );
    assert_eq!(cues_heard(&cues, 1), ["Audio ducking."]);
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "duck_level", "value": 40}),
    );
    assert_eq!(cues_heard(&cues, 1), ["Duck level 40 percent."]);
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "mute_level", "value": 1}),
    );
    assert_eq!(cues_heard(&cues, 1), ["Muted."]);
    // Unchanged: no cue.
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "mute_level", "value": 1}),
    );
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "mute_level", "value": 0}),
    );
    assert_eq!(cues_heard(&cues, 1), ["Unmuted."]);
    // A rate set from a page is not announced (Python: only the keys).
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "rate", "value": 300}),
    );
    assert!(cues.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn the_cues_event_stream_needs_the_system_extension() {
    let r = rig();
    let s = &r.server;
    let mut t = Session::tcp();
    ok(s, &mut t, json!({"type": "hello", "token": TOKEN}));
    let e = call(s, &mut t, json!({"type": "subscribe", "events": ["cues"]}));
    assert_eq!(e["error"]["code"], "E_UNSUPPORTED");
    ok(
        s,
        &mut t,
        json!({"type": "hello", "token": TOKEN, "extensions": ["system"]}),
    );
    let o = s.handle(&mut t, &json!({"type": "subscribe", "events": ["cues"]}));
    assert_eq!(o.reply["events"], json!(["cues"]));
    let sonarad::protocol::After::Subscribe(mut rx) = o.after else {
        panic!("expected a subscription");
    };
    let mut h = Session::http();
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "audio_mode", "value": "duck"}),
    );
    let w = rx.blocking_recv().unwrap();
    assert_eq!(w.name, "cue");
    let v: Value = serde_json::from_str(&w.json).unwrap();
    assert_eq!(v, json!({"event": "cue", "text": "Audio ducking."}));
}

#[test]
fn a_muted_channel_pref_holds_the_channels_speech() {
    // #196: session_prefs muted is enforced, also after a restart.
    let home = tmp();
    {
        let r = rig_on(home.clone());
        let s = &r.server;
        let mut h = Session::http();
        ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
        // Read prose at once (the product default holds the turn).
        ok(
            s,
            &mut h,
            json!({"type": "set", "key": "read_mode", "value": "immediate"}),
        );
        ok(
            s,
            &mut h,
            json!({"type": "set", "key": "channel_prefs", "value": {"channel": "m", "muted": true}}),
        );
        let channels = s.channels().unwrap().clone();
        assert!(channels.is_muted("m"));
        ok(
            s,
            &mut h,
            json!({"type": "stream", "channel": "m", "delta": "Held.", "final": true}),
        );
        std::thread::sleep(Duration::from_millis(100));
        assert!(s.reader().state().unwrap().now_playing.is_none());
        ok(
            s,
            &mut h,
            json!({"type": "set", "key": "channel_prefs", "value": {"channel": "m", "muted": false}}),
        );
        assert!(eventually(|| s
            .reader()
            .state()
            .unwrap()
            .now_playing
            .is_some()));
        ok(
            s,
            &mut h,
            json!({"type": "set", "key": "channel_prefs", "value": {"channel": "m", "muted": true}}),
        );
    }
    let r = rig_on(home.to_path_buf());
    let s = &r.server;
    let mut h = Session::http();
    ok(
        s,
        &mut h,
        json!({"type": "hello", "extensions": ["channels"]}),
    );
    assert!(s.channels().unwrap().is_muted("m"), "kept across a restart");
}

#[test]
fn forgetting_a_channel_drops_its_prefs_channel_and_turn() {
    let r = rig();
    let s = &r.server;
    let mut h = Session::http();
    ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
    ok(
        s,
        &mut h,
        json!({"type": "channel_open", "channel": "dead", "label": "x"}),
    );
    ok(
        s,
        &mut h,
        json!({"type": "channel_open", "channel": "live"}),
    );
    ok(s, &mut h, json!({"type": "focus", "channel": "live"}));
    ok(
        s,
        &mut h,
        json!({"type": "turn_start", "channel": "dead", "t": 50.0}),
    );
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "channel_prefs", "value": {"channel": "dead", "label": "Old"}}),
    );
    let e = call(
        s,
        &mut h,
        json!({"type": "set", "key": "channel_prefs", "value": {"channel": "live", "forget": true}}),
    );
    assert_eq!(
        e["error"]["code"], "E_BAD_REQUEST",
        "the focused channel stays"
    );
    let g = ok(
        s,
        &mut h,
        json!({"type": "set", "key": "channel_prefs", "value": {"channel": "dead", "forget": true}}),
    );
    let ids: Vec<&str> = g["value"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["channel"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["live"]);
    assert!(s.channels().unwrap().channel("dead").is_none());
    assert!(s.agent().unwrap().tracked().is_empty());
}

#[test]
fn the_background_policy_is_a_persisted_agent_key() {
    let home = tmp();
    {
        let r = rig_on(home.clone());
        let s = &r.server;
        let mut h = Session::http();
        ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
        assert!(!s.channels().unwrap().focus_only(), "all by default (#202)");
        let g = ok(
            s,
            &mut h,
            json!({"type": "set", "key": "background_policy", "value": "earcon_only"}),
        );
        assert_eq!(g["value"], "earcon_only");
        assert!(s.channels().unwrap().focus_only());
        assert_eq!(saved(&home)["background_policy"], "earcon_only");
    }
    let r = rig_on(home.to_path_buf());
    let s = &r.server;
    let mut h = Session::http();
    ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
    assert!(s.channels().unwrap().focus_only(), "applied at start");
}

#[test]
fn the_read_mode_is_a_persisted_agent_key_and_an_old_minqueue_maps_to_it() {
    // #222.
    let home = tmp();
    {
        let r = rig_on(home.clone());
        let s = &r.server;
        let mut h = Session::http();
        ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
        let g = ok(s, &mut h, json!({"type": "get", "key": "read_mode"}));
        assert_eq!(g["value"], "done", "the product default");
        let g = ok(
            s,
            &mut h,
            json!({"type": "set", "key": "read_mode", "value": "queue"}),
        );
        assert_eq!(g["value"], "queue");
        assert_eq!(saved(&home)["read_mode"], "queue");
    }
    {
        let r = rig_on(home.clone());
        let s = &r.server;
        let mut h = Session::http();
        ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
        let g = ok(s, &mut h, json!({"type": "get", "key": "read_mode"}));
        assert_eq!(g["value"], "queue", "applied at start");
    }
    // A config.json from before #222: the minqueue the user set.
    let home = tmp();
    std::fs::write(home.join("config.json"), r#"{"minqueue": 1}"#).unwrap();
    let r = rig_on(home.clone());
    let s = &r.server;
    let mut h = Session::http();
    ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
    let g = ok(s, &mut h, json!({"type": "get", "key": "read_mode"}));
    assert_eq!(g["value"], "immediate");
}

/// Kokoro as `sonarad` builds it, but without downloads, ONNX Runtime or a
/// model: it speaks with its fallback (the fake engine) and lists its voices.
fn offline_kokoro(home: &std::path::Path) -> sonara_engine::kokoro::Kokoro {
    use sonara_engine::kokoro::{self, download};
    let mut c = kokoro::Config::new(
        home.join("models").join(download::MODEL_SUBDIR),
        home.join("no-onnxruntime.dll"),
    );
    c.download = false;
    c.fallback = Some(Arc::new(FakeEngine::new()));
    kokoro::Kokoro::new(c)
}

#[test]
fn saved_kokoro_settings_apply_and_previews_use_the_readers_kokoro() {
    use sonara_reader::{Key, Value as V};
    let home = tmp();
    std::fs::write(
        home.join("config.json"),
        r#"{"engine": "kokoro", "voice": "af_sarah", "rate": 250}"#,
    )
    .unwrap();
    let k = offline_kokoro(&home);
    let registry = Registry::default();
    registry.register(Arc::new(k.clone())).unwrap();
    let mut config = Config::new(registry);
    config.engine = Some("kokoro".into());
    let (out, rx) = TestOutput::new();
    let reader = ReaderHandle::new(config.with_output(Box::new(out.clone()), rx)).unwrap();
    let (store, problems) = Store::load(&home);
    assert!(problems.is_empty(), "{problems:?}");
    let problems = config::apply_reader(&store, &reader, false);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(reader.get(Key::Voice).unwrap(), V::Text("af_sarah".into()));
    assert_eq!(reader.get(Key::Rate).unwrap(), V::Number(250));
    // The previews share the reader's Kokoro (one model, one download).
    let previews = Registry::default();
    previews.register(Arc::new(k)).unwrap();
    let server = Server::new(
        reader,
        TOKEN.into(),
        Lifetime::new(Duration::from_secs(30), false),
    )
    .with_config(store)
    .with_system(SystemHost {
        platform: Fake::new().platform(),
        home: home.clone(),
        http_port: 4321,
        token: TOKEN.into(),
        previews: Some(Arc::new(previews)),
    });
    let s = &server;
    let mut h = Session::http();
    ok(
        s,
        &mut h,
        json!({"type": "hello", "extensions": ["system"]}),
    );
    let p = ok(s, &mut h, json!({"type": "preview"}));
    assert_eq!(p["engine"], "kokoro");
    assert_eq!(
        p["voice"], "af_sarah",
        "the saved voice is the one in force"
    );
    let p = ok(s, &mut h, json!({"type": "preview", "voice": "bm_george"}));
    assert_eq!(p["voice"], "bm_george");
    assert!(
        out.take_calls().iter().any(
            |c| matches!(c, sonara_audio::OutputCall::PlayClip { samples, .. } if *samples > 0)
        ),
        "the previews played"
    );
    let g = ok(s, &mut h, json!({"type": "get", "key": "runtime"}));
    assert_eq!(g["value"]["saved_voice"], "af_sarah");
    // #214: Kokoro without its model is not ready yet.
    assert_eq!(g["value"]["engine_status"]["engine"], "kokoro");
    assert_eq!(g["value"]["engine_status"]["ready"], false);
}

#[test]
fn shutdown_stops_reading_refuses_more_and_asks_to_exit() {
    // #202: `sonara stop` and upgrades end the runtime through `shutdown`.
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    let refused = s.handle(&mut session, &json!({"type": "shutdown"}));
    assert_eq!(refused.reply["ok"], false, "not before the extension is on");
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system"]}),
    );
    ok(
        s,
        &mut session,
        json!({"type": "speak", "text": "Hello there."}),
    );
    let out = s.handle(&mut session, &json!({"type": "shutdown"}));
    assert_eq!(out.reply["ok"], true, "{}", out.reply);
    assert!(matches!(out.after, sonarad::protocol::After::Exit));
    let busy = call(s, &mut session, json!({"type": "speak", "text": "More."}));
    assert_eq!(busy["error"]["code"], "E_BUSY", "{busy}");
}

#[test]
fn the_flush_hotkey_stops_only_the_session_being_read() {
    // #228: the flush key cut the session being read and also wiped the
    // turn another session was still streaming.
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system", "agent"], "keep_alive": true}),
    );
    ok(
        s,
        &mut session,
        json!({"type": "set", "key": "read_mode", "value": "done"}),
    );
    for (id, label) in [("a", "alpha-repo"), ("b", "beta-repo")] {
        ok(
            s,
            &mut session,
            json!({"type": "channel_open", "channel": id, "label": label}),
        );
    }
    ok(
        s,
        &mut session,
        json!({"type": "stream", "channel": "b", "delta": "Beta still arriving.", "final": false}),
    );
    ok(
        s,
        &mut session,
        json!({"type": "stream", "channel": "a", "delta": "Alpha one. Alpha two.", "final": true}),
    );
    ok(s, &mut session, json!({"type": "turn_end", "channel": "a"}));
    assert!(eventually(|| playing(s).as_deref() == Some("Alpha one.")));
    start_playing(&r.out);
    r.fake.press(Action::Flush.id());
    assert!(eventually(
        || log_of(&r).contains("hotkey flush session=alpha-repo")
    ));
    assert!(eventually(|| playing(s).is_none()));
    ok(
        s,
        &mut session,
        json!({"type": "stream", "channel": "b", "delta": " Done now.", "final": true}),
    );
    ok(s, &mut session, json!({"type": "turn_end", "channel": "b"}));
    // The switch announcement, then b's whole turn.
    assert!(eventually(
        || playing(s).is_some_and(|t| t.contains("beta-repo"))
    ));
    start_playing(&r.out);
    r.out.finish();
    assert!(eventually(
        || playing(s).as_deref() == Some("Beta still arriving.")
    ));
    let log = log_of(&r);
    assert!(log.contains("reason=flush"), "{log}");
    assert!(
        log.contains("agent flush channel=a wipe reason=flush"),
        "{log}"
    );
}

#[test]
fn the_flush_hotkey_with_nothing_being_read_is_logged_idle() {
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system", "agent"], "keep_alive": true}),
    );
    r.fake.press(Action::Flush.id());
    assert!(eventually(|| log_of(&r).contains("hotkey flush idle")));
}

#[test]
fn the_flush_scope_is_a_persisted_agent_key() {
    // #228 (2026-10-04): what the flush hotkey skips, "session" by default.
    let home = tmp();
    {
        let r = rig_on(home.clone());
        let s = &r.server;
        let mut h = Session::http();
        ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
        let g = ok(s, &mut h, json!({"type": "get", "key": "flush_scope"}));
        assert_eq!(g["value"], "session", "the product default");
        let g = ok(
            s,
            &mut h,
            json!({"type": "set", "key": "flush_scope", "value": "all"}),
        );
        assert_eq!(g["value"], "all");
        assert_eq!(saved(&home)["flush_scope"], "all");
        let bad = call(
            s,
            &mut h,
            json!({"type": "set", "key": "flush_scope", "value": "everything"}),
        );
        assert_eq!(bad["error"]["code"], "E_BAD_REQUEST", "{bad}");
    }
    let r = rig_on(home.clone());
    let s = &r.server;
    let mut h = Session::http();
    ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
    let g = ok(s, &mut h, json!({"type": "get", "key": "flush_scope"}));
    assert_eq!(g["value"], "all", "applied at start");
}

#[test]
fn the_flush_hotkey_with_scope_all_drops_every_ready_message_and_keeps_a_reply_still_arriving() {
    // #228: scope all skips the session being read and every finished
    // reply waiting; a session still writing its reply keeps it.
    let r = rig();
    let s = &r.server;
    let mut session = Session::http();
    ok(
        s,
        &mut session,
        json!({"type": "hello", "extensions": ["system", "agent"], "keep_alive": true}),
    );
    for (key, value) in [("read_mode", "done"), ("flush_scope", "all")] {
        ok(
            s,
            &mut session,
            json!({"type": "set", "key": key, "value": value}),
        );
    }
    for (id, label) in [("a", "alpha-repo"), ("b", "beta-repo"), ("c", "gamma-repo")] {
        ok(
            s,
            &mut session,
            json!({"type": "channel_open", "channel": id, "label": label}),
        );
        ok(
            s,
            &mut session,
            json!({"type": "turn_start", "channel": id}),
        );
    }
    ok(
        s,
        &mut session,
        json!({"type": "stream", "channel": "c", "delta": "Gamma still arriving.", "final": false}),
    );
    for id in ["a", "b"] {
        ok(
            s,
            &mut session,
            json!({"type": "stream", "channel": id, "delta": "Finished reply.", "final": true}),
        );
        ok(s, &mut session, json!({"type": "turn_end", "channel": id}));
    }
    assert!(eventually(
        || playing(s).as_deref() == Some("Finished reply.")
    ));
    start_playing(&r.out);
    r.fake.press(Action::Flush.id());
    assert!(eventually(|| log_of(&r).contains(
        "hotkey flush session=alpha-repo scope=all others=beta-repo"
    )));
    assert!(eventually(|| playing(s).is_none()));
    ok(
        s,
        &mut session,
        json!({"type": "stream", "channel": "c", "delta": " Done now.", "final": true}),
    );
    ok(s, &mut session, json!({"type": "turn_end", "channel": "c"}));
    assert!(eventually(
        || playing(s).is_some_and(|t| t.contains("gamma-repo"))
    ));
    start_playing(&r.out);
    r.out.finish();
    assert!(eventually(
        || playing(s).as_deref() == Some("Gamma still arriving.")
    ));
}

#[test]
fn control_flush_tells_the_scope_and_the_other_sessions() {
    let r = rig();
    let s = &r.server;
    let mut h = Session::http();
    ok(s, &mut h, json!({"type": "hello", "extensions": ["agent"]}));
    let f = ok(s, &mut h, json!({"type": "control", "action": "flush"}));
    assert_eq!(f["flushed"], "nothing");
    assert_eq!(f["scope"], "session");
    assert_eq!(f["others"], json!([]));
    ok(
        s,
        &mut h,
        json!({"type": "set", "key": "flush_scope", "value": "all"}),
    );
    let f = ok(s, &mut h, json!({"type": "control", "action": "flush"}));
    assert_eq!(f["scope"], "all");
}
