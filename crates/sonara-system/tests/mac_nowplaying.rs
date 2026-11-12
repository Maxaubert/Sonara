//! The Now Playing pause scripts, run by the real `/usr/bin/osascript`
//! against a fake MediaRemote backend (JavaScript with the real one's
//! shape): the targeting decisions are the script's own, checked here
//! without touching this Mac's media. The real backend is only read
//! (`the_real_read_never_hangs`); commands to it are `#[ignore]` live
//! tests in `mac_live.rs`.
#![cfg(target_os = "macos")]

use serde_json::{json, Value};
use sonara_system::mac::{osascript, MEDIA_APPS};
use sonara_system::mac_nowplaying::{NowPlayingMedia, Runner, Snapshot};
use sonara_system::platform::{timed_out, MediaSession};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A fake backend: `now` is what plays (or null); a sent command changes
/// its state as the app would and is counted in `sends`.
fn fake_backend(now: Value, send_ok: bool) -> String {
    format!(
        r#"
var fakeNow = {now};
var sends = [];
function backend() {{
  return {{
    snap: function () {{
      return fakeNow === null ? null : {{app: fakeNow.app, title: fakeNow.title,
        state: fakeNow.state, isPlaying: fakeNow.isPlaying, sends: sends.slice()}};
    }},
    send: function (cmd) {{
      sends.push(cmd);
      if (!{send_ok}) return false;
      fakeNow.state = cmd === 1 ? 2 : 1;
      fakeNow.isPlaying = cmd === 0;
      return true;
    }},
    wait: function (s) {{}}
  }};
}}
"#
    )
}

/// Real osascript; the last raw answer is kept for the checks.
fn runner() -> (Runner, Arc<Mutex<Value>>) {
    let last = Arc::new(Mutex::new(Value::Null));
    let keep = last.clone();
    let run: Runner = Arc::new(move |script: &str| {
        let out = osascript(script, true, Duration::from_secs(10))?;
        *keep.lock().unwrap() = serde_json::from_str(&out).unwrap_or(Value::Null);
        Ok(out)
    });
    (run, last)
}

fn media(now: Value, send_ok: bool) -> (NowPlayingMedia, Arc<Mutex<Value>>) {
    let (run, last) = runner();
    let backend = fake_backend(now, send_ok);
    (NowPlayingMedia::with(run, &backend, MEDIA_APPS), last)
}

fn chrome(state: i64) -> Value {
    json!({"app": "com.google.Chrome", "title": "A video", "state": state, "isPlaying": state == 1})
}

/// A session for `want` on a backend where `now` plays.
fn session(now: Value, want: Value, send_ok: bool) -> (Box<dyn MediaSession>, Arc<Mutex<Value>>) {
    let (m, last) = media(now, send_ok);
    let want: Snapshot = serde_json::from_value(want).unwrap();
    let s = m.sessions_from(Ok(Some(want))).pop().expect("a session");
    (s, last)
}

fn sends(last: &Arc<Mutex<Value>>) -> Value {
    let v = last.lock().unwrap();
    v["after"]["sends"].clone()
}

#[test]
fn the_playing_now_playing_app_is_listed_and_paused_with_the_pause_command() {
    let (m, _) = media(chrome(1), true);
    let s = m.sessions();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].app_id().unwrap(), "com.google.Chrome");
    assert_eq!(s[0].is_playing(), Ok(true));

    let (s, last) = session(chrome(1), chrome(1), true);
    assert_eq!(s.pause(), Ok(()));
    assert_eq!(sends(&last), json!([1]), "kMRPause only");
    assert_eq!(last.lock().unwrap()["after"]["state"], 2);
}

#[test]
fn media_that_is_not_playing_is_never_paused() {
    let (s, last) = session(chrome(2), chrome(1), true);
    assert!(s.pause().is_err());
    assert_eq!(last.lock().unwrap()["sent"], false);
    assert_eq!(last.lock().unwrap()["tried"], false);
}

#[test]
fn other_media_now_playing_is_never_paused() {
    let safari =
        json!({"app": "com.apple.Safari", "title": "Other", "state": 1, "isPlaying": true});
    let (s, last) = session(safari, chrome(1), true);
    assert!(s.pause().unwrap_err().contains("no longer"));
    assert_eq!(last.lock().unwrap()["tried"], false);
}

#[test]
fn resume_plays_only_the_paused_media_sonara_paused() {
    let (s, last) = session(chrome(2), chrome(1), true);
    assert_eq!(s.play(), Ok(()));
    assert_eq!(sends(&last), json!([0]), "kMRPlay only");
}

#[test]
fn resume_never_starts_other_media_or_media_that_plays_or_stopped() {
    let other = json!({"app": "com.google.Chrome", "title": "Another video", "state": 2, "isPlaying": false});
    for now in [other, chrome(1), chrome(3), Value::Null] {
        let (s, last) = session(now.clone(), chrome(1), true);
        assert_eq!(s.play(), Ok(()), "nothing to resume is no error: {now}");
        assert_eq!(last.lock().unwrap()["tried"], false, "{now}");
    }
}

#[test]
fn a_command_mediaremote_refuses_is_an_error() {
    let (s, _) = session(chrome(1), chrome(1), false);
    assert!(s.pause().is_err());
    let (s, _) = session(chrome(2), chrome(1), false);
    assert!(s.play().is_err(), "retried by the pauser");
}

#[test]
fn a_backend_that_fails_to_load_is_no_session() {
    let (run, _) = runner();
    let m = NowPlayingMedia::with(
        run,
        "function backend() { return {error: 'MediaRemote did not load'}; }",
        MEDIA_APPS,
    );
    assert!(m.read().unwrap_err().contains("did not load"));
    assert!(m.sessions().is_empty());
    let (run, _) = runner();
    let m = NowPlayingMedia::with(
        run,
        "function backend() { throw new Error('boom'); }",
        MEDIA_APPS,
    );
    assert!(m.sessions().is_empty());
}

#[test]
fn a_hung_script_is_killed_at_the_timeout() {
    let run: Runner = Arc::new(|script: &str| osascript(script, true, Duration::from_millis(500)));
    let m = NowPlayingMedia::with(run, "function backend() { while (true) {} }", MEDIA_APPS);
    let started = Instant::now();
    let e = m.read().unwrap_err();
    assert!(timed_out(&e), "{e}");
    assert!(m.sessions().is_empty());
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// Read only (no command): MediaRemote through osascript answers, or fails,
/// within the call timeout. It may be unavailable (a CI runner), so only
/// the bound is checked.
#[test]
fn the_real_read_never_hangs() {
    let started = Instant::now();
    let read = NowPlayingMedia::new().read();
    println!("now playing: {read:?}");
    assert!(started.elapsed() < Duration::from_secs(4));
}
