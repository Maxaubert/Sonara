//! The engage and restore rules of `AudioControl` (other apps' audio while
//! L1 reads), on the fake platform: engage while an item is read, restore
//! at once on pause, mute, a mode change and disarm (a client that needed
//! the extension left), after a grace when idle, and on shutdown; the
//! startup sweep restores what a killed runtime left.
mod common;

use common::{eventually, tmp};
use serde_json::json;
use sonara_audio::TestOutput;
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config, Control, QueueMode, ReaderHandle, Registry};
use sonara_system::audio::{Activity, AudioConfig, AudioControl, AudioMode, DUCK_STATE};
use sonara_system::fake::Fake;
use std::sync::Arc;
use std::time::Duration;

const OWN_PID: u32 = 4242;

fn control(fake: &Fake, grace: Duration) -> (AudioControl, std::path::PathBuf) {
    let dir = tmp("audio");
    let c = AudioControl::new(
        &fake.platform(),
        AudioConfig {
            state_dir: dir.clone(),
            exclude_pids: vec![OWN_PID],
            idle_grace: grace,
            log: None,
        },
    );
    (c, dir)
}

fn vol(fake: &Fake, pid: u32) -> f32 {
    fake.volume(pid).unwrap()
}

fn ducked_rig(grace: Duration) -> (Fake, AudioControl, std::path::PathBuf) {
    let fake = Fake::new();
    fake.add_audio(100, "vlc.exe", 0.8);
    fake.add_audio(OWN_PID, "sonarad.exe", 1.0);
    let (c, dir) = control(&fake, grace);
    c.set_mode(AudioMode::Duck);
    c.set_duck_level(25);
    c.arm(true);
    c.set_activity(Activity::Speaking);
    c.sync();
    (fake, c, dir)
}

#[test]
fn speech_ducks_other_apps_and_a_pause_restores_at_once() {
    let (fake, c, _) = ducked_rig(Duration::from_secs(60));
    assert!((vol(&fake, 100) - 0.25).abs() < 1e-4);
    assert_eq!(
        vol(&fake, OWN_PID),
        1.0,
        "the runtime's own audio is never ducked"
    );
    assert!(c.status().ducked);
    c.set_activity(Activity::Held);
    c.sync();
    assert!((vol(&fake, 100) - 0.8).abs() < 1e-4);
    assert!(!c.status().ducked);
}

#[test]
fn idle_restores_after_the_grace_and_a_new_message_within_it_keeps_the_duck() {
    let (fake, c, _) = ducked_rig(Duration::from_millis(300));
    c.set_activity(Activity::Idle);
    c.sync();
    assert!(c.status().ducked, "the gap between messages keeps the duck");
    c.set_activity(Activity::Speaking);
    c.sync();
    let calls = fake.world().audio[0].set_calls;
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        fake.world().audio[0].set_calls,
        calls,
        "never restored and re-ducked"
    );
    c.set_activity(Activity::Idle);
    assert!(eventually(|| !c.status().ducked));
    assert!((vol(&fake, 100) - 0.8).abs() < 1e-4);
}

#[test]
fn mode_off_or_disarmed_never_engages() {
    let fake = Fake::new();
    fake.add_audio(100, "vlc.exe", 0.8);
    let (c, _) = control(&fake, Duration::ZERO);
    c.arm(true);
    c.set_activity(Activity::Speaking);
    c.sync();
    assert_eq!(vol(&fake, 100), 0.8, "mode off");
    c.arm(false);
    c.set_mode(AudioMode::Duck);
    c.sync();
    assert_eq!(vol(&fake, 100), 0.8, "not armed");
    c.arm(true);
    c.sync();
    assert!((vol(&fake, 100) - 0.3).abs() < 1e-4, "default level 30");
}

#[test]
fn restore_on_client_drop() {
    // Disarming (the last client that enabled the extension left) restores
    // at once, even while the reader keeps reading.
    let (fake, c, dir) = ducked_rig(Duration::from_secs(60));
    assert!(dir.join(DUCK_STATE).exists());
    c.arm(false);
    c.sync();
    assert!((vol(&fake, 100) - 0.8).abs() < 1e-4);
    assert!(!dir.join(DUCK_STATE).exists());
}

#[test]
fn pause_mode_pauses_media_and_resumes_it_when_speech_ends() {
    let fake = Fake::new();
    fake.add_media("spotify", true);
    fake.add_media("game", false);
    let (c, _) = control(&fake, Duration::ZERO);
    c.set_mode(AudioMode::Pause);
    c.arm(true);
    c.set_activity(Activity::Speaking);
    c.sync();
    assert!(!fake.media("spotify").unwrap().playing);
    assert!(c.status().paused);
    c.set_activity(Activity::Idle);
    c.sync();
    assert!(fake.media("spotify").unwrap().playing);
    assert_eq!(fake.media("game").unwrap().plays, 0);
}

#[test]
fn a_mode_switch_while_speaking_never_leaves_the_other_backend_engaged() {
    let (fake, c, _) = ducked_rig(Duration::from_secs(60));
    fake.add_media("spotify", true);
    c.set_mode(AudioMode::Pause);
    c.sync();
    assert!((vol(&fake, 100) - 0.8).abs() < 1e-4, "duck restored");
    assert!(
        !fake.media("spotify").unwrap().playing,
        "now paused instead"
    );
    c.set_mode(AudioMode::Off);
    c.sync();
    assert!(fake.media("spotify").unwrap().playing);
    assert!(!c.status().ducked && !c.status().paused);
}

#[test]
fn a_new_duck_level_is_applied_while_ducked() {
    let (fake, c, _) = ducked_rig(Duration::from_secs(60));
    c.set_duck_level(10);
    c.sync();
    assert!((vol(&fake, 100) - 0.1).abs() < 1e-4);
    c.set_activity(Activity::Held);
    c.sync();
    assert!(
        (vol(&fake, 100) - 0.8).abs() < 1e-4,
        "the true original survives"
    );
}

#[test]
fn shutdown_restores() {
    let (fake, c, dir) = ducked_rig(Duration::from_secs(60));
    c.shutdown();
    assert!((vol(&fake, 100) - 0.8).abs() < 1e-4);
    assert!(!dir.join(DUCK_STATE).exists());
    c.set_activity(Activity::Speaking); // after shutdown: nothing happens
}

#[test]
fn restore_after_runtime_kill() {
    // A killed runtime leaves the crash record; the next runtime's startup
    // sweep restores both ducked sessions and paused media.
    let fake = Fake::new();
    fake.add_audio(100, "vlc.exe", 0.2);
    fake.add_media("spotify", false);
    let (c, dir) = control(&fake, Duration::ZERO);
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
    assert!((vol(&fake, 100) - 0.9).abs() < 1e-4);
    assert!(fake.media("spotify").unwrap().playing);
    assert!(!dir.join(DUCK_STATE).exists());
    assert!(!dir.join("pause_state.json").exists());
}

#[test]
fn activity_follows_the_reader_state() {
    let registry = Registry::default();
    registry.register(Arc::new(FakeEngine::new())).unwrap();
    let (out, rx) = TestOutput::new();
    let reader =
        ReaderHandle::new(Config::new(registry).with_output(Box::new(out.clone()), rx)).unwrap();
    let fake = Fake::new();
    fake.add_audio(100, "vlc.exe", 0.8);
    let (c, _) = control(&fake, Duration::ZERO);
    let c = Arc::new(c);
    c.set_mode(AudioMode::Duck);
    c.arm(true);
    c.follow(reader.subscribe().unwrap(), None);
    reader
        .speak("One. Two.", QueueMode::Append, false, None)
        .unwrap();
    assert!(eventually(|| c.status().ducked), "{:?}", c.status());
    reader.control(Control::Pause).unwrap();
    assert!(eventually(|| !c.status().ducked));
    reader.control(Control::Play).unwrap();
    assert!(eventually(|| c.status().ducked));
    reader.control(Control::Mute).unwrap();
    assert!(eventually(|| !c.status().ducked), "muted speech is silent");
    reader.control(Control::Unmute).unwrap();
    assert!(eventually(|| c.status().ducked));
    reader.control(Control::Stop).unwrap();
    assert!(eventually(|| !c.status().ducked));
    assert!((vol(&fake, 100) - 0.8).abs() < 1e-4);
    reader.shutdown();
}

#[test]
fn following_mid_item_engages_at_once() {
    // The system extension may be enabled while an item is already being
    // read: the current state counts, not only the next change.
    let registry = Registry::default();
    registry.register(Arc::new(FakeEngine::new())).unwrap();
    let (out, rx) = TestOutput::new();
    let reader =
        ReaderHandle::new(Config::new(registry).with_output(Box::new(out.clone()), rx)).unwrap();
    reader
        .speak("One. Two.", QueueMode::Append, false, None)
        .unwrap();
    assert!(eventually(|| reader
        .state()
        .is_ok_and(|s| s.now_playing.is_some())));
    let fake = Fake::new();
    fake.add_audio(100, "vlc.exe", 0.8);
    let (c, _) = control(&fake, Duration::ZERO);
    let c = Arc::new(c);
    c.set_mode(AudioMode::Duck);
    c.arm(true);
    c.follow(reader.subscribe().unwrap(), reader.state().ok().as_ref());
    assert!(eventually(|| c.status().ducked), "{:?}", c.status());
    reader.control(Control::Stop).unwrap();
    assert!(eventually(|| !c.status().ducked));
    reader.shutdown();
}
