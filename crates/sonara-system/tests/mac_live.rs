//! Live checks of the macOS platform against this Mac. They register the
//! user's real global hotkeys for a moment, ask a person to press one, or
//! talk to the running media players (the first call to a player shows
//! macOS's Automation prompt once) or to the Now Playing app (a video in
//! a browser; no prompt), so they are `#[ignore]`: run them by
//! hand with
//! `cargo build -p sonara-system --example mac_hotkeys` and then
//! `cargo test -p sonara-system --test mac_live -- --ignored --nocapture`.
//!
//! Hotkeys fire only on the main thread's event pump, which a test thread
//! is not, so those checks run the `mac_hotkeys` example (it pumps on its
//! main thread, as sonarad does).
#![cfg(target_os = "macos")]

use sonara_system::mac;
use sonara_system::mac_nowplaying::NowPlayingMedia;
use std::path::PathBuf;
use std::process::Command;

/// The `mac_hotkeys` example next to this test binary.
fn probe() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().and_then(|d| d.parent()).unwrap();
    let probe = dir.join("examples").join("mac_hotkeys");
    assert!(
        probe.is_file(),
        "build it first: cargo build -p sonara-system --example mac_hotkeys"
    );
    probe
}

#[test]
#[ignore = "registers this Mac's global hotkeys for a moment"]
fn registers_and_releases_the_default_hotkeys_on_the_main_thread_pump() {
    let out = Command::new(probe()).arg("register").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    println!("{text}");
    assert!(out.status.success());
    assert!(text.contains("registered 4"), "{text}");
    assert!(text.contains("Ctrl+Option+Up"), "{text}");
    assert!(text.contains("released"), "{text}");
}

#[test]
#[ignore = "needs a person to press Ctrl+Option+Up: a tap, then a hold"]
fn a_real_press_arrives_as_a_tap_and_a_hold() {
    let out = Command::new(probe())
        .args(["listen", "15"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    println!("{text}");
    assert!(text.contains("pressed restart Tap"), "{text}");
    assert!(text.contains("pressed restart Hold"), "{text}");
}

#[test]
#[ignore = "talks to this Mac's running media players"]
fn lists_running_media_players_and_their_state() {
    let sessions = (mac::platform().media)().sessions().expect("osascript");
    for s in &sessions {
        println!("{:?} playing {:?}", s.app_id(), s.is_playing());
    }
}

#[test]
#[ignore = "pauses this Mac's playing media player for a second"]
fn pauses_and_resumes_a_playing_player() {
    let media = (mac::platform().media)();
    let playing: Vec<_> = media
        .sessions()
        .expect("osascript")
        .into_iter()
        .filter(|s| s.is_playing() == Ok(true))
        .collect();
    assert!(
        !playing.is_empty(),
        "start Music, Spotify or a video in a browser first"
    );
    for s in &playing {
        s.pause().unwrap();
        assert_eq!(s.is_playing(), Ok(false));
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    for s in &playing {
        s.play().unwrap();
        assert_eq!(s.is_playing(), Ok(true), "never left paused");
    }
}

#[test]
#[ignore = "reads this Mac's Now Playing media (no command)"]
fn reads_the_now_playing_app_through_osascript() {
    let now = NowPlayingMedia::new()
        .read()
        .expect("MediaRemote through osascript");
    println!("now playing: {now:?}");
}

/// Play a YouTube video in Chrome (or any app that shows in Now Playing)
/// first: it is paused for a second, then resumed.
#[test]
#[ignore = "pauses this Mac's Now Playing media for a second"]
fn pauses_and_resumes_the_now_playing_app() {
    let media = NowPlayingMedia::new();
    let sessions = media.sessions();
    let s = sessions
        .first()
        .expect("play a video in a browser (not Music or Spotify) first");
    println!("now playing: {:?}", s.app_id());
    assert_eq!(s.is_playing(), Ok(true), "it must be playing");
    s.pause().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(s.is_playing(), Ok(false), "paused");
    std::thread::sleep(std::time::Duration::from_secs(1));
    s.play().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(s.is_playing(), Ok(true), "never left paused");
}
