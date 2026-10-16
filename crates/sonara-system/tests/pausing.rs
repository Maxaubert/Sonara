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
    // What a previous runtime left is known by app id only: resuming it
    // needs the session list.
    let (fake, mut p, state) = setup("keepall");
    std::fs::write(&state, json!({"apps": ["spotify"]}).to_string()).unwrap();
    fake.add_media("spotify", false);
    fake.edit(|w| w.media_down = true);
    p.recover();
    p.resume();
    assert!(!p.is_paused());
    assert_eq!(read_json(&state)["apps"], json!(["spotify"]));
    fake.edit(|w| w.media_down = false);
    p.resume();
    assert!(fake.media("spotify").unwrap().playing);
    assert!(!state.exists());
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
    assert_eq!(read_json(&state)["apps"], json!(["vlc"]));
}

#[test]
fn what_could_not_be_resumed_is_retried_and_survives_the_next_pause() {
    let (fake, mut p, state) = setup("leftover");
    std::fs::write(&state, json!({"apps": ["vlc"]}).to_string()).unwrap();
    fake.add_media("vlc", false);
    fake.add_media("spotify", true);
    fake.edit(|w| w.media[0].fail_play = true);
    p.recover();
    assert_eq!(read_json(&state)["apps"], json!(["vlc"]));
    p.pause();
    assert_eq!(read_json(&state)["apps"], json!(["vlc", "spotify"]));
    fake.edit(|w| w.media[0].fail_play = false);
    p.resume();
    assert!(fake.media("vlc").unwrap().playing);
    assert!(fake.media("spotify").unwrap().playing);
    assert!(!state.exists());
}

#[test]
fn resume_plays_only_the_sessions_the_pause_paused() {
    // #17: two tabs of one browser share an app id; the one the user had
    // paused stays paused.
    let (fake, mut p, _) = setup("tabs");
    fake.add_media("chrome", true);
    fake.add_media("chrome", false);
    p.pause();
    p.resume();
    let w = fake.world();
    assert!(w.media[0].playing);
    assert_eq!(w.media[1].plays, 0, "the paused tab is not played");
    assert!(!w.media[1].playing);
}

#[test]
fn the_sweep_drops_a_record_older_than_the_age_limit() {
    // #17: an old record (a crash, then a reboot days later) never starts
    // an app's media unasked.
    let (fake, _, state) = setup("old");
    std::fs::write(&state, json!({"apps": ["spotify"], "at": 1}).to_string()).unwrap();
    fake.add_media("spotify", false);
    resume_from_state_file((fake.platform().media)().as_ref(), &state);
    assert_eq!(fake.media("spotify").unwrap().plays, 0);
    assert!(!state.exists());
}

#[test]
fn a_pending_resume_skips_a_session_that_plays_again() {
    // #17: a resume that failed is retried only while the session is still
    // paused; one the user started again is left alone.
    let (fake, mut p, state) = setup("replay");
    fake.add_media("vlc", true);
    fake.add_media("spotify", true);
    p.pause();
    fake.edit(|w| w.media[0].fail_play = true);
    p.resume();
    assert_eq!(read_json(&state)["apps"], json!(["vlc"]));
    fake.edit(|w| {
        w.media[0].fail_play = false;
        w.media[0].playing = true; // the user starts vlc by hand
    });
    let plays = fake.media("vlc").unwrap().plays;
    p.resume();
    assert_eq!(fake.media("vlc").unwrap().plays, plays);
    assert!(!state.exists());
}

#[test]
fn a_pause_that_times_out_is_still_resumed() {
    // #16: an app that answers late may pause after all, so a timed-out
    // pause is resumed like a paused one.
    let (fake, mut p, _) = setup("late");
    fake.add_media("spotify", true);
    fake.edit(|w| w.media[0].pause_times_out = true);
    p.pause();
    assert!(!fake.media("spotify").unwrap().playing);
    p.resume();
    assert!(fake.media("spotify").unwrap().playing);
}
