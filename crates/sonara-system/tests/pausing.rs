//! Media pausing rules, ported from the Python `tests/test_pausing.py`
//! (including L-pause-state) onto the fake platform.
mod common;

use common::{read_json, tmp};
use serde_json::json;
use sonara_system::fake::Fake;
use sonara_system::pausing::{resume_from_state_file, MediaPauser};
use std::path::PathBuf;

fn setup(tag: &str) -> (Fake, MediaPauser, PathBuf) {
    let fake = Fake::new();
    let state = tmp(tag).join("pause_state.json");
    let p = MediaPauser::new((fake.platform().media)(), state.clone());
    (fake, p, state)
}

#[test]
fn pause_pauses_only_playing_sessions() {
    let (fake, mut p, _) = setup("only");
    fake.add_media("spotify", true);
    fake.add_media("game", false);
    p.pause();
    assert!(p.is_paused());
    assert_eq!(fake.media("spotify").unwrap().pauses, 1);
    assert_eq!(fake.media("game").unwrap().pauses, 0);
}

#[test]
fn resume_plays_only_what_the_pause_paused() {
    let (fake, mut p, _) = setup("resume");
    fake.add_media("spotify", true);
    fake.add_media("game", false);
    p.pause();
    p.resume();
    assert!(!p.is_paused());
    assert!(fake.media("spotify").unwrap().playing);
    assert_eq!(fake.media("game").unwrap().plays, 0);
}

#[test]
fn resume_skips_a_session_that_vanished() {
    let (fake, mut p, state) = setup("vanished");
    fake.add_media("a", true);
    fake.add_media("b", true);
    p.pause();
    fake.edit(|w| w.media.retain(|m| m.app != "a"));
    p.resume();
    assert!(fake.media("b").unwrap().playing);
    assert!(!state.exists());
}

#[test]
fn pause_writes_the_crash_record_and_resume_clears_it() {
    let (fake, mut p, state) = setup("file");
    fake.add_media("spotify", true);
    p.pause();
    assert_eq!(read_json(&state)["apps"], json!(["spotify"]));
    p.resume();
    assert!(!state.exists());
}

#[test]
fn pause_survives_media_that_cannot_be_listed() {
    let (fake, mut p, _) = setup("down");
    fake.add_media("spotify", true);
    fake.edit(|w| w.media_down = true);
    p.pause();
    assert!(!p.is_paused());
}

#[test]
fn a_failed_resume_keeps_the_app_in_the_crash_record() {
    // Never leave other apps paused: what could not be resumed is left for
    // the startup sweep.
    let (fake, mut p, state) = setup("keep");
    fake.add_media("spotify", true);
    fake.add_media("vlc", true);
    p.pause();
    fake.edit(|w| w.media[1].fail_play = true);
    p.resume();
    assert!(fake.media("spotify").unwrap().playing);
    assert_eq!(read_json(&state)["apps"], json!(["vlc"]));
}

#[test]
fn a_resume_that_cannot_list_sessions_keeps_every_app() {
    let (fake, mut p, state) = setup("keepall");
    fake.add_media("spotify", true);
    p.pause();
    fake.edit(|w| w.media_down = true);
    p.resume();
    assert!(!p.is_paused());
    assert_eq!(read_json(&state)["apps"], json!(["spotify"]));
}

#[test]
fn the_sweep_plays_recorded_apps_and_deletes_the_file() {
    let (fake, _, state) = setup("sweep");
    std::fs::write(&state, json!({"apps": ["spotify"]}).to_string()).unwrap();
    fake.add_media("spotify", false);
    resume_from_state_file((fake.platform().media)().as_ref(), &state);
    assert!(fake.media("spotify").unwrap().playing);
    assert!(!state.exists());
}

#[test]
fn the_sweep_keeps_the_file_when_media_cannot_be_listed() {
    let (fake, _, state) = setup("sweepdown");
    std::fs::write(&state, json!({"apps": ["spotify"]}).to_string()).unwrap();
    fake.edit(|w| w.media_down = true);
    resume_from_state_file((fake.platform().media)().as_ref(), &state);
    assert_eq!(read_json(&state), json!({"apps": ["spotify"]}));
}

#[test]
fn the_sweep_keeps_only_the_apps_that_failed() {
    let (fake, _, state) = setup("sweeppart");
    std::fs::write(&state, json!({"apps": ["spotify", "vlc"]}).to_string()).unwrap();
    fake.add_media("spotify", false);
    fake.add_media("vlc", false);
    fake.edit(|w| w.media[1].fail_play = true);
    resume_from_state_file((fake.platform().media)().as_ref(), &state);
    assert!(fake.media("spotify").unwrap().playing);
    assert_eq!(read_json(&state), json!({"apps": ["vlc"]}));
}

#[test]
fn what_could_not_be_resumed_is_retried_and_survives_the_next_pause() {
    let (fake, mut p, state) = setup("leftover");
    std::fs::write(&state, json!({"apps": ["vlc"]}).to_string()).unwrap();
    fake.add_media("vlc", false);
    fake.add_media("spotify", true);
    fake.edit(|w| w.media[0].fail_play = true);
    p.recover();
    assert_eq!(read_json(&state), json!({"apps": ["vlc"]}));
    p.pause();
    assert_eq!(read_json(&state), json!({"apps": ["vlc", "spotify"]}));
    fake.edit(|w| w.media[0].fail_play = false);
    p.resume();
    assert!(fake.media("vlc").unwrap().playing);
    assert!(fake.media("spotify").unwrap().playing);
    assert!(!state.exists());
}
