//! #227: no request of any kind reaches an external engine while Sonara is
//! muted (core `mute`) or at agent `mute_level` 1 or 2: messages, lookahead,
//! cues ("Muted.", "Unmuted.", "Rate 225.") and voice lists are spoken or
//! answered locally. Muting cuts a request in flight. The user's own Test
//! button and voice preview still reach the provider, and everything
//! resumes once unmuted. A local server stands in for the provider and
//! counts what it receives.
mod common;

use common::Provider;
use serde_json::{json, Value};
use sonara_audio::{OutputCall, TestOutput};
use sonara_engine::external::keys::MemoryStore;
use sonara_engine::fake::FakeEngine;
use sonara_engine::{Engine, LicenseClass};
use sonara_reader::{Config, ReaderHandle, Registry};
use sonara_system::fake::Fake;
use sonara_system::keymap::Action;
use sonarad::config::Store;
use sonarad::engines::{self, Engines};
use sonarad::lifetime::Lifetime;
use sonarad::protocol::{Server, Session};
use sonarad::system_ext::SystemHost;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SECRET: &str = "sk-test-secret-0123456789abcdef";

struct Rig {
    server: Server,
    out: TestOutput,
    session: Session,
    provider: Provider,
    /// What the external engine speaks with when it may not (or cannot)
    /// use the provider.
    fallback: Arc<FakeEngine>,
    fake: Fake,
    home: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn registry(classes: &[LicenseClass]) -> Arc<Registry> {
    let r = Registry::new(classes);
    r.register(Arc::new(FakeEngine::new())).unwrap();
    Arc::new(r)
}

/// A server with the external engine `local` current, the `system`
/// extension (cues, previews, hotkeys on the fake platform) and, with
/// `agent`, the agent extension.
fn rig(agent: bool) -> Rig {
    static N: AtomicU32 = AtomicU32::new(0);
    let home = std::env::temp_dir().join(format!(
        "sonarad-muted-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let classes = [
        LicenseClass::Permissive,
        LicenseClass::Os,
        LicenseClass::External,
    ];
    let reader_engines = registry(&classes);
    let preview_engines = registry(&classes);
    let fallback = Arc::new(FakeEngine::new());
    let (engines, _) = Engines::load(engines::Setup {
        home: home.clone(),
        store: Arc::new(MemoryStore::new()),
        fallback: Some(fallback.clone() as Arc<dyn Engine>),
        fallback_voice: String::new(),
        default_engine: "fake".into(),
        log: None,
    });
    engines.attach(reader_engines.clone());
    engines.attach(preview_engines.clone());
    let (out, rx) = TestOutput::new();
    let reader =
        ReaderHandle::new(Config::new(reader_engines).with_output(Box::new(out.clone()), rx))
            .unwrap();
    let fake = Fake::new();
    let (store, _) = Store::load(&home);
    let server = Server::new(
        reader,
        "secret".into(),
        Lifetime::new(Duration::from_secs(30), false),
    )
    .with_config(store)
    .with_engines(engines)
    .with_system(SystemHost {
        platform: fake.platform(),
        home: home.clone(),
        http_port: 4321,
        token: "secret".into(),
        previews: Some(preview_engines),
    });
    let mut session = Session::tcp();
    let mut extensions = vec!["system"];
    if agent {
        extensions.push("agent");
    }
    let o = server.handle(
        &mut session,
        &json!({"type": "hello", "token": "secret", "extensions": extensions,
                "keep_alive": true}),
    );
    assert_eq!(o.reply["ok"], true, "{}", o.reply);
    let mut r = Rig {
        server,
        out,
        session,
        provider: Provider::start(),
        fallback,
        fake,
        home,
    };
    let p = json!({"id": "local", "kind": "openai-compatible", "label": "Local",
        "url": r.provider.url, "key_ref": "credman",
        "options": {"preset": "kokoro-fastapi", "timeout_ms": 5000}});
    r.ok(json!({"type": "engine_add", "engine": p, "secret": SECRET}));
    r.ok(json!({"type": "set", "key": "engine", "value": "local"}));
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

impl Rig {
    fn call(&mut self, req: Value) -> Value {
        self.server.handle(&mut self.session, &req).reply
    }

    fn ok(&mut self, req: Value) -> Value {
        let r = self.call(req.clone());
        assert_eq!(r["ok"], true, "{req} -> {r}");
        r
    }

    /// Every request the provider saw (speech and voice lists).
    fn requests(&self) -> usize {
        self.provider.seen.lock().unwrap().len()
    }

    fn speech(&self) -> usize {
        self.provider.speech_requests().len()
    }

    /// Play the item through: each chunk starts and finishes as it loads.
    fn play_through(&self) {
        let reader = self.server.reader().clone();
        let end = Instant::now() + Duration::from_secs(10);
        while reader.state().unwrap().now_playing.is_some() {
            assert!(Instant::now() < end, "the item never ended");
            if self.out.loaded().is_some() {
                self.out.start();
                self.out.finish();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn clips(&self) -> usize {
        self.out
            .calls()
            .iter()
            .filter(|c| matches!(c, OutputCall::PlayClip { .. }))
            .count()
    }

    fn cues(&self) -> sonarad::cues::CueStream {
        self.server.system().unwrap().cues().subscribe()
    }
}

fn heard(cues: &sonarad::cues::CueStream, text: &str) {
    let got = cues.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(got, text);
}

const MESSAGE: &str = "First sentence here. Second sentence here. Third sentence here.";

#[test]
fn a_message_while_core_muted_sends_nothing() {
    let mut r = rig(false);
    r.ok(json!({"type": "control", "action": "mute"}));
    r.ok(json!({"type": "speak", "text": MESSAGE}));
    r.play_through();
    assert_eq!(r.requests(), 0, "nothing reached the provider");
    assert_eq!(r.fallback.texts().len(), 3, "read locally, every chunk");
    // Unmuted, the provider reads again.
    r.ok(json!({"type": "control", "action": "unmute"}));
    r.ok(json!({"type": "speak", "text": "Back again."}));
    r.play_through();
    assert_eq!(r.speech(), 1, "requests resume after unmute");
}

#[test]
fn a_message_at_mute_level_1_or_2_sends_nothing() {
    for level in [1, 2] {
        let mut r = rig(true);
        r.ok(json!({"type": "set", "key": "mute_level", "value": level}));
        // Agent text is not spoken; a core speak is read, but locally.
        r.ok(json!({"type": "stream", "channel": "a", "delta": MESSAGE, "final": true}));
        r.ok(json!({"type": "turn_end", "channel": "a"}));
        r.ok(json!({"type": "speak", "text": MESSAGE}));
        r.play_through();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(r.requests(), 0, "mute_level {level}");
        assert!(r.fallback.texts().iter().any(|t| t.starts_with("First")));
        r.ok(json!({"type": "set", "key": "mute_level", "value": 0}));
        r.ok(json!({"type": "speak", "text": "Back again."}));
        r.play_through();
        assert_eq!(r.speech(), 1, "requests resume at level 0");
    }
}

#[test]
fn muting_mid_message_cuts_the_request_and_fetches_nothing_more() {
    for how in ["control", "mute_level"] {
        let mut r = rig(how == "mute_level");
        *r.provider.delay.lock().unwrap() = Duration::from_secs(3);
        r.ok(json!({"type": "speak", "text": MESSAGE}));
        assert!(eventually(|| r.speech() == 1), "the first chunk is asked");
        let at = Instant::now();
        if how == "control" {
            r.ok(json!({"type": "control", "action": "mute"}));
            // Core mute keeps playback moving silently: the chunk cut is
            // read locally, and so are the ones after it.
            assert!(eventually(|| r.out.loaded().is_some()));
            assert!(
                at.elapsed() < Duration::from_secs(2),
                "the request in flight was cut, not waited for"
            );
            r.play_through();
        } else {
            r.ok(json!({"type": "set", "key": "mute_level", "value": 1}));
            assert!(eventually(|| r
                .server
                .reader()
                .state()
                .unwrap()
                .now_playing
                .is_none()));
        }
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(r.speech(), 1, "{how}: nothing after the mute");
    }
}

#[test]
fn mute_and_unmute_cues_never_reach_the_provider() {
    let mut r = rig(true);
    let cues = r.cues();
    r.ok(json!({"type": "set", "key": "mute_level", "value": 1}));
    heard(&cues, "Muted.");
    r.ok(json!({"type": "set", "key": "mute_level", "value": 2}));
    heard(&cues, "Super muted.");
    // A rate cue while muted is local too.
    r.ok(json!({"type": "set", "key": "hotkeys",
                "value": {"action": "faster", "key": "]", "mods": ["ctrl", "alt"]}}));
    r.fake.press(Action::Faster.id());
    heard(&cues, "Rate 225.");
    r.ok(json!({"type": "set", "key": "mute_level", "value": 0}));
    heard(&cues, "Unmuted.");
    assert_eq!(r.requests(), 0, "every cue was spoken locally");
    assert!(eventually(|| r.clips() >= 4), "and heard as clips");
    for t in ["Muted.", "Super muted.", "Rate 225.", "Unmuted."] {
        assert!(r.fallback.texts().iter().any(|x| x == t), "{t}");
    }
    // The hotkey's mute cycle too.
    r.ok(json!({"type": "set", "key": "hotkeys",
                "value": {"action": "mute", "key": "m", "mods": ["ctrl", "alt"]}}));
    for want in ["Muted.", "Super muted.", "Unmuted."] {
        std::thread::sleep(sonara_system::hotkeys::DEBOUNCE + Duration::from_millis(50));
        r.fake.press(Action::Mute.id());
        heard(&cues, want);
    }
    assert_eq!(r.requests(), 0);
    // Unmuted, a cue is the provider's again.
    std::thread::sleep(sonara_system::hotkeys::DEBOUNCE + Duration::from_millis(50));
    r.fake.press(Action::Faster.id());
    heard(&cues, "Rate 250.");
    assert_eq!(r.speech(), 1);
}

#[test]
fn the_core_mute_hotkey_and_its_cues_stay_local() {
    let mut r = rig(false);
    let cues = r.cues();
    r.ok(json!({"type": "set", "key": "hotkeys",
                "value": {"action": "mute", "key": "m", "mods": ["ctrl", "alt"]}}));
    r.fake.press(Action::Mute.id());
    heard(&cues, "Muted.");
    assert!(r.server.reader().state().unwrap().muted);
    r.ok(json!({"type": "speak", "text": MESSAGE}));
    r.play_through();
    std::thread::sleep(sonara_system::hotkeys::DEBOUNCE + Duration::from_millis(50));
    r.fake.press(Action::Mute.id());
    heard(&cues, "Unmuted.");
    assert_eq!(r.requests(), 0);
}

#[test]
fn voice_lists_are_not_fetched_while_muted() {
    let mut r = rig(false);
    r.ok(json!({"type": "control", "action": "mute"}));
    let o = r.ok(json!({"type": "voices", "engine": "local", "refresh": true}));
    assert_eq!(o["error"]["reason"], "muted", "{o}");
    let p = json!({"kind": "openai-compatible", "url": r.provider.url, "key_ref": "credman",
                   "options": {"preset": "kokoro-fastapi"}});
    let o = r.ok(json!({"type": "voices", "profile": p, "secret": SECRET}));
    assert_eq!(o["error"]["reason"], "muted", "{o}");
    assert_eq!(r.requests(), 0);
    r.ok(json!({"type": "control", "action": "unmute"}));
    let o = r.ok(json!({"type": "voices", "engine": "local", "refresh": true}));
    assert!(o.get("error").is_none(), "{o}");
    assert_eq!(r.requests(), 1);
}

#[test]
fn the_test_button_and_a_voice_preview_still_reach_the_provider() {
    let mut r = rig(true);
    r.ok(json!({"type": "set", "key": "mute_level", "value": 2}));
    r.ok(json!({"type": "control", "action": "mute"}));
    r.ok(json!({"type": "engine_test", "engine": "local", "play": false}));
    assert_eq!(r.speech(), 1, "Test is the user's own action");
    r.ok(json!({"type": "preview", "voice": "af_sky"}));
    assert_eq!(r.speech(), 2, "so is a preview");
}

#[test]
fn a_persisted_mute_level_holds_from_the_start() {
    let mut r = rig(false);
    // A runtime whose saved mute_level is 1: the agent starts muted.
    r.server.store().record("mute_level", &json!(1));
    let mut s = Session::tcp();
    let o = r.server.handle(
        &mut s,
        &json!({"type": "hello", "token": "secret", "extensions": ["agent"]}),
    );
    assert_eq!(o.reply["ok"], true);
    r.ok(json!({"type": "speak", "text": MESSAGE}));
    r.play_through();
    assert_eq!(r.requests(), 0);
}
