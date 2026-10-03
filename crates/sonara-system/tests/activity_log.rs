//! The activity lines of `AudioControl` (#217): one line per engage and
//! restore that touched an app, with the apps and why (the item read and
//! its session label), failures, and the startup sweep. No line when no
//! app was touched.
mod common;

use common::{eventually, tmp};
use serde_json::json;
use sonara_audio::TestOutput;
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config, Control, QueueMode, ReaderHandle, Registry};
use sonara_system::audio::{Activity, AudioConfig, AudioControl, AudioMode, DUCK_STATE};
use sonara_system::fake::Fake;
use sonara_system::LogFn;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const OWN_PID: u32 = 4242;

#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    fn sink(&self) -> LogFn {
        let l = self.0.clone();
        Arc::new(move |line: &str| l.lock().unwrap().push(line.to_string()))
    }

    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    fn has(&self, prefix: &str) -> bool {
        self.all().iter().any(|l| l.starts_with(prefix))
    }
}

fn control(fake: &Fake, lines: &Lines) -> (AudioControl, std::path::PathBuf) {
    let dir = tmp("activity");
    let c = AudioControl::new(
        &fake.platform(),
        AudioConfig {
            state_dir: dir.clone(),
            exclude_pids: vec![OWN_PID],
            idle_grace: Duration::ZERO,
            log: None,
        }
        .with_log(Some(lines.sink())),
    );
    (c, dir)
}

fn reader() -> ReaderHandle {
    let mut registry = Registry::default();
    registry.register(Arc::new(FakeEngine::new())).unwrap();
    let (out, rx) = TestOutput::new();
    ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap()
}

#[test]
fn a_pause_names_the_apps_and_the_item_and_session_read() {
    let fake = Fake::new();
    fake.add_media("spotify", true);
    fake.add_media("game", false);
    let lines = Lines::default();
    let (c, _) = control(&fake, &lines);
    let c = Arc::new(c);
    c.set_mode(AudioMode::Pause);
    c.arm(true);
    let r = reader();
    c.follow(r.subscribe().unwrap(), None);
    let id = r
        .speak(
            "One. Two. Three.",
            QueueMode::Append,
            false,
            Some("my work".into()),
        )
        .unwrap();
    assert!(eventually(|| lines.has("media pause")), "{:?}", lines.all());
    r.control(Control::Stop).unwrap();
    assert!(
        eventually(|| lines.has("media resume")),
        "{:?}",
        lines.all()
    );
    assert_eq!(
        lines.all(),
        vec![
            format!(
                "media pause apps=spotify (reason: reading item={} session=\"my work\")",
                id.0
            ),
            "media resume apps=spotify (reason: idle)".to_string(),
        ]
    );
    r.shutdown();
}

#[test]
fn duck_and_restore_lines_say_why() {
    let fake = Fake::new();
    fake.add_audio(100, "vlc.exe", 0.8);
    fake.add_audio(OWN_PID, "sonarad.exe", 1.0);
    let lines = Lines::default();
    let (c, _) = control(&fake, &lines);
    c.set_mode(AudioMode::Duck);
    c.set_duck_level(25);
    c.arm(true);
    c.set_activity_because(Activity::Speaking, Some("reading item=7".into()));
    c.sync();
    c.set_duck_level(40);
    c.sync();
    c.set_activity(Activity::Held);
    c.sync();
    c.set_activity(Activity::Speaking);
    c.sync();
    c.arm(false);
    c.sync();
    assert_eq!(
        lines.all(),
        vec![
            "duck apps=vlc.exe level=25 (reason: reading item=7)",
            "duck apps=vlc.exe level=40 (reason: level change)",
            "restore apps=vlc.exe (reason: paused or muted)",
            "duck apps=vlc.exe level=40 (reason: reading item=7)",
            "restore apps=vlc.exe (reason: disarmed)",
        ]
    );
}

#[test]
fn a_mode_switch_and_shutdown_are_named() {
    let fake = Fake::new();
    fake.add_audio(100, "vlc.exe", 0.8);
    fake.add_media("spotify", true);
    let lines = Lines::default();
    let (c, _) = control(&fake, &lines);
    c.set_mode(AudioMode::Duck);
    c.arm(true);
    c.set_activity_because(Activity::Speaking, Some("reading item=1".into()));
    c.sync();
    c.set_mode(AudioMode::Pause);
    c.sync();
    c.shutdown();
    assert_eq!(
        lines.all(),
        vec![
            "duck apps=vlc.exe level=30 (reason: reading item=1)",
            "restore apps=vlc.exe (reason: mode pause)",
            "media pause apps=spotify (reason: reading item=1)",
            "media resume apps=spotify (reason: shutdown)",
        ]
    );
}

#[test]
fn nothing_touched_writes_no_line() {
    let fake = Fake::new();
    fake.add_media("game", false);
    let lines = Lines::default();
    let (c, _) = control(&fake, &lines);
    c.set_mode(AudioMode::Pause);
    c.arm(true);
    c.set_activity(Activity::Speaking);
    c.sync();
    c.set_activity(Activity::Idle);
    c.sync();
    c.shutdown();
    assert!(lines.all().is_empty(), "{:?}", lines.all());
}

#[test]
fn a_failed_resume_is_logged_with_the_app() {
    let fake = Fake::new();
    fake.add_media("spotify", true);
    let lines = Lines::default();
    let (c, _) = control(&fake, &lines);
    c.set_mode(AudioMode::Pause);
    c.arm(true);
    c.set_activity(Activity::Speaking);
    c.sync();
    fake.edit(|w| w.media[0].fail_play = true);
    c.set_activity(Activity::Idle);
    c.sync();
    let all = lines.all();
    assert!(
        all.contains(&"media resume failed apps=spotify".to_string()),
        "{all:?}"
    );
    assert!(
        all.contains(&"media resume apps=spotify (reason: idle)".to_string()),
        "{all:?}"
    );
    fake.edit(|w| w.media[0].fail_play = false);
}

#[test]
fn the_startup_sweep_names_what_it_restored() {
    let fake = Fake::new();
    fake.add_audio(100, "vlc.exe", 0.2);
    fake.add_media("spotify", false);
    let lines = Lines::default();
    let (c, dir) = control(&fake, &lines);
    std::fs::write(
        dir.join(DUCK_STATE),
        json!({"sessions": [{"pid": 100, "name": "vlc.exe", "original": 0.9}]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("pause_state.json"),
        json!({"apps": ["spotify"]}).to_string(),
    )
    .unwrap();
    c.recover();
    assert_eq!(
        lines.all(),
        vec![
            "startup sweep: restore apps=vlc.exe",
            "startup sweep: media resume apps=spotify",
        ]
    );
    // A clean start (no crash files) logs nothing.
    let lines = Lines::default();
    let (c, _) = control(&fake, &lines);
    c.recover();
    assert!(lines.all().is_empty(), "{:?}", lines.all());
}
