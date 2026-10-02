//! Ducking rules, ported from the Python `tests/test_ducking.py`
//! (including #130 and #131) onto the fake platform.
mod common;

use common::{read_json, tmp};
use serde_json::json;
use sonara_system::ducking::{restore_from_state_file, Ducker};
use sonara_system::fake::Fake;
use std::path::PathBuf;

fn setup(tag: &str) -> (Fake, Ducker, PathBuf) {
    let fake = Fake::new();
    let state = tmp(tag).join("duck_state.json");
    let ducker = Ducker::new((fake.platform().audio)(), state.clone());
    (fake, ducker, state)
}

fn approx(a: Option<f32>, b: f32) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-4)
}

fn write_state(path: &PathBuf, v: serde_json::Value) {
    std::fs::write(path, v.to_string()).unwrap();
}

#[test]
fn duck_lowers_other_sessions_to_the_level() {
    let (fake, mut d, _) = setup("lower");
    fake.add_audio(100, "a.exe", 0.8);
    fake.add_audio(200, "b.exe", 0.6);
    d.duck(&[], 20);
    assert!(d.is_ducked());
    assert!(approx(fake.volume(100), 0.2));
    assert!(approx(fake.volume(200), 0.2));
}

#[test]
fn duck_skips_excluded_pids() {
    let (fake, mut d, _) = setup("exclude");
    fake.add_audio(999, "sonarad.exe", 0.9);
    fake.add_audio(100, "vlc.exe", 0.8);
    d.duck(&[999], 20);
    assert!(approx(fake.volume(999), 0.9), "own process untouched");
    assert!(approx(fake.volume(100), 0.2));
}

#[test]
fn duck_never_touches_the_audio_engine_or_virtual_routers() {
    // Their session is the whole mix, Sonara's own speech included.
    let (fake, mut d, _) = setup("never");
    fake.add_audio(500, "audiodg.exe", 0.9);
    fake.add_audio(501, "SteelSeriesSonar.exe", 0.9);
    fake.add_audio(600, "firefox.exe", 0.8);
    d.duck(&[], 20);
    assert!(approx(fake.volume(500), 0.9));
    assert!(approx(fake.volume(501), 0.9), "matched case-insensitively");
    assert!(approx(fake.volume(600), 0.2));
}

#[test]
fn restore_puts_the_original_volumes_back() {
    let (fake, mut d, _) = setup("restore");
    fake.add_audio(100, "vlc.exe", 0.7);
    d.duck(&[], 10);
    assert!(approx(fake.volume(100), 0.1));
    d.restore();
    assert!(approx(fake.volume(100), 0.7));
    assert!(!d.is_ducked());
}

#[test]
fn a_second_duck_is_a_no_op() {
    let (fake, mut d, _) = setup("idem");
    fake.add_audio(100, "vlc.exe", 0.8);
    d.duck(&[], 20);
    fake.edit(|w| w.audio[0].volume = 0.5); // someone else changed it
    d.duck(&[], 20);
    assert!(approx(fake.volume(100), 0.5));
}

#[test]
fn duck_writes_the_crash_record_and_restore_clears_it() {
    let (fake, mut d, state) = setup("file");
    fake.add_audio(100, "vlc.exe", 0.8);
    d.duck(&[], 20);
    let rec = read_json(&state);
    assert_eq!(rec["sessions"][0]["pid"], 100);
    assert_eq!(rec["sessions"][0]["name"], "vlc.exe");
    assert!((rec["sessions"][0]["original"].as_f64().unwrap() - 0.8).abs() < 1e-4);
    d.restore();
    assert!(!state.exists());
}

#[test]
fn duck_survives_sessions_that_cannot_be_listed_and_retries() {
    let (fake, mut d, state) = setup("down");
    fake.add_audio(100, "vlc.exe", 0.8);
    fake.edit(|w| w.audio_down = true);
    d.duck(&[], 20);
    assert!(!d.is_ducked(), "nothing lowered: the next call retries");
    d.restore();
    assert!(!state.exists());
    fake.edit(|w| w.audio_down = false);
    d.duck(&[], 20);
    assert!(approx(fake.volume(100), 0.2));
}

#[test]
fn a_partial_duck_is_recorded_and_restored() {
    // #130: a later session failing never strands the ones already lowered.
    let (fake, mut d, state) = setup("partial");
    fake.add_audio(100, "zen.exe", 1.0);
    fake.add_audio(200, "bad.exe", 1.0);
    fake.edit(|w| w.audio[1].broken = true);
    d.duck(&[], 30);
    assert!(approx(fake.volume(100), 0.3));
    assert!(d.is_ducked());
    let names: Vec<_> = read_json(&state)["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].clone())
        .collect();
    assert_eq!(names, vec![json!("zen.exe")]);
    d.restore();
    assert!(approx(fake.volume(100), 1.0));
    assert!(!state.exists());
}

#[test]
fn duck_never_saves_an_already_ducked_level_as_the_original() {
    let (fake, mut d, _) = setup("stuck");
    fake.add_audio(100, "zen.exe", 0.3);
    d.duck(&[], 30);
    assert!(
        d.saved().is_empty(),
        "0.3 is never recorded as the original"
    );
}

#[test]
fn a_failed_restore_is_retried_by_a_fresh_lookup() {
    let (fake, mut d, state) = setup("retry");
    fake.add_audio(100, "zen.exe", 1.0);
    // The duck is set call 1; the first restore (call 2) fails once.
    fake.edit(|w| w.audio[0].fail_set_on = Some(2));
    d.duck(&[], 30);
    d.restore();
    assert!(approx(fake.volume(100), 1.0));
    assert!(!state.exists());
    assert!(!d.is_ducked());
}

#[test]
fn an_unrecoverable_restore_keeps_the_record_through_the_next_duck() {
    let (fake, mut d, state) = setup("pending");
    fake.add_audio(100, "zen.exe", 1.0);
    d.duck(&[], 30);
    fake.edit(|w| w.audio[0].broken = true); // every restore attempt fails
    d.restore();
    assert!(!d.is_ducked());
    let rec = read_json(&state);
    assert_eq!(rec["sessions"][0]["name"], "zen.exe");
    assert_eq!(rec["sessions"][0]["original"], 1.0);
    // The device comes back at the ducked level; the next duck skips it
    // (already at the level) but keeps the pending record, so its real
    // original survives.
    fake.edit(|w| {
        w.audio[0].broken = false;
        w.audio[0].volume = 0.3;
    });
    d.duck(&[], 30);
    assert_eq!(read_json(&state)["sessions"][0]["original"], 1.0);
    d.restore();
    assert!(approx(fake.volume(100), 1.0));
    assert!(!state.exists());
}

#[test]
fn the_sweep_restores_matching_live_sessions() {
    let (fake, _, state) = setup("sweep");
    write_state(
        &state,
        json!({"sessions": [{"pid": 100, "name": "vlc.exe", "original": 0.9}]}),
    );
    fake.add_audio(100, "vlc.exe", 0.2);
    restore_from_state_file((fake.platform().audio)().as_ref(), &state);
    assert!(approx(fake.volume(100), 0.9));
    assert!(!state.exists());
}

#[test]
fn the_sweep_keeps_the_file_when_sessions_cannot_be_listed() {
    let (fake, _, state) = setup("sweepdown");
    write_state(
        &state,
        json!({"sessions": [{"pid": 1, "name": "x.exe", "original": 0.5}]}),
    );
    fake.edit(|w| w.audio_down = true);
    restore_from_state_file((fake.platform().audio)().as_ref(), &state);
    assert!(state.exists(), "nothing restored: keep the record (#130)");
}

#[test]
fn the_sweep_keeps_only_the_entries_that_failed() {
    let (fake, _, state) = setup("sweeppart");
    write_state(
        &state,
        json!({"sessions": [
            {"pid": 100, "name": "zen.exe", "original": 1.0},
            {"pid": 200, "name": "vlc.exe", "original": 0.8}]}),
    );
    fake.add_audio(100, "zen.exe", 0.3);
    fake.add_audio(200, "vlc.exe", 0.3);
    fake.edit(|w| w.audio[0].broken = true);
    restore_from_state_file((fake.platform().audio)().as_ref(), &state);
    assert!(approx(fake.volume(200), 0.8));
    let left = read_json(&state)["sessions"].clone();
    assert_eq!(left.as_array().unwrap().len(), 1);
    assert_eq!(left[0]["name"], "zen.exe");
}

#[test]
fn the_sweep_ignores_a_reused_pid_of_another_app() {
    let (fake, _, state) = setup("reused");
    write_state(
        &state,
        json!({"sessions": [{"pid": 100, "name": "vlc.exe", "original": 0.9}]}),
    );
    fake.add_audio(100, "game.exe", 0.5); // pid 100 reused
    fake.add_audio(300, "vlc.exe", 0.2); // vlc restarted on a new pid
    restore_from_state_file((fake.platform().audio)().as_ref(), &state);
    assert!(approx(fake.volume(100), 0.5), "the stranger is untouched");
    assert!(approx(fake.volume(300), 0.9));
}

#[test]
fn the_sweep_matches_a_pid_with_no_recorded_name() {
    let (fake, _, state) = setup("noname");
    write_state(&state, json!({"sessions": [{"pid": 100, "original": 0.9}]}));
    fake.add_audio(100, "vlc.exe", 0.2);
    restore_from_state_file((fake.platform().audio)().as_ref(), &state);
    assert!(approx(fake.volume(100), 0.9));
}

#[test]
fn the_sweep_drops_a_record_whose_app_is_gone() {
    let (fake, _, state) = setup("gone");
    write_state(
        &state,
        json!({"sessions": [{"pid": 100, "name": "vlc.exe", "original": 0.9}]}),
    );
    fake.add_audio(7, "other.exe", 0.4);
    restore_from_state_file((fake.platform().audio)().as_ref(), &state);
    assert!(!state.exists());
}

#[test]
fn a_session_is_recorded_before_it_is_lowered() {
    // A runtime killed between lowering a session and writing the record
    // would strand that app: the record is on disk before the volume moves.
    let (fake, mut d, state) = setup("order");
    fake.add_audio(100, "vlc.exe", 0.8);
    fake.add_audio(200, "zen.exe", 0.9);
    // The second session's lowering fails, as if the runtime died there:
    // the first one is on disk already, the second one never was lowered.
    fake.edit(|w| w.audio[1].fail_set_on = Some(1));
    let probe = state.clone();
    d.duck(&[], 20);
    let rec = read_json(&probe);
    let pids: Vec<_> = rec["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["pid"].clone())
        .collect();
    assert_eq!(
        pids,
        vec![json!(100)],
        "only what was lowered stays recorded"
    );
    // The record of each session exists while it is being lowered.
    let (fake, mut d, state) = setup("order2");
    fake.add_audio(100, "vlc.exe", 0.8);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let s2 = seen.clone();
    let st = state.clone();
    fake.on_set_volume(move || {
        *s2.lock().unwrap() = Some(st.exists());
    });
    d.duck(&[], 20);
    assert_eq!(*seen.lock().unwrap(), Some(true));
}

#[test]
fn restore_after_runtime_kill() {
    // A runtime killed while ducked never restores; the next one's startup
    // sweep does, from the crash record (Review Focus 5).
    let (fake, mut d, state) = setup("kill");
    fake.add_audio(100, "vlc.exe", 0.8);
    d.duck(&[], 20);
    std::mem::forget(d); // killed: no restore, no drop
    assert!(approx(fake.volume(100), 0.2));
    restore_from_state_file((fake.platform().audio)().as_ref(), &state);
    assert!(approx(fake.volume(100), 0.8));
    assert!(!state.exists());
}

#[test]
fn what_the_sweep_could_not_restore_survives_the_next_duck() {
    // A failed startup sweep leaves records; a later duck must not
    // overwrite them, and the next restore retries them.
    let (fake, mut d, state) = setup("leftover");
    write_state(
        &state,
        json!({"sessions": [{"pid": 300, "name": "zen.exe", "original": 1.0}]}),
    );
    fake.add_audio(300, "zen.exe", 0.3);
    fake.add_audio(100, "vlc.exe", 0.8);
    fake.edit(|w| w.audio[0].broken = true);
    d.recover();
    assert_eq!(d.pending().len(), 1);
    d.duck(&[], 20);
    let names: Vec<_> = read_json(&state)["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].clone())
        .collect();
    assert_eq!(names, vec![json!("zen.exe"), json!("vlc.exe")]);
    fake.edit(|w| w.audio[0].broken = false);
    d.restore();
    assert!(approx(fake.volume(300), 1.0));
    assert!(approx(fake.volume(100), 0.8));
    assert!(!state.exists());
}
