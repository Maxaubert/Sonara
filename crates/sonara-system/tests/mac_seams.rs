//! The macOS seams that touch nothing of the user's: the Carbon event pump
//! with hotkey events posted to the queue (no key is pressed and no chord
//! registered: a recording backend stands in for `RegisterEventHotKey`),
//! tap and hold from the release event, the key translation, the bounded
//! `osascript` call (#16) and the SIGTERM path (#18, in a child process).
//! Not `#[ignore]`: none of them needs a person or touches an app.
#![cfg(target_os = "macos")]

use sonara_system::hotkeys::{Hotkeys, HOLD};
use sonara_system::keymap::{
    Action, Resolved, MOD_ALT, MOD_CTRL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};
use sonara_system::mac::{self, Backend, Pump};
use sonara_system::platform::{timed_out, KeyState};
use sonara_system::Press;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the recording backend was asked to register: (vk, mods, id).
type Calls = Arc<Mutex<Vec<(u32, u32, u32)>>>;

struct Recording(Calls);

impl Backend for Recording {
    fn register(&mut self, vk: u32, mods: u32, id: u32) -> Result<usize, u32> {
        let mut calls = self.0.lock().unwrap();
        calls.push((vk, mods, id));
        Ok(calls.len())
    }

    fn unregister(&mut self, _token: usize) {}
}

/// Set in a child this binary runs (`in_child`).
const CHILD: &str = "SONARA_SEAM_CHILD";

/// Run the `#[ignore]`d test `name` in a child process of this binary and
/// assert it passed. Carbon binds the event queue to the first thread that
/// pumps it, so every test that pumps gets a fresh process (with
/// `cargo test` the tests share one; a runtime has one pump, on its main
/// thread). Returns the child's exit status for tests that expect another.
fn in_child(name: &str, env: &[(&str, &std::ffi::OsStr)]) -> std::process::ExitStatus {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", name, "--ignored", "--test-threads=1"])
        .env(CHILD, "1")
        .stdout(std::process::Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let start = Instant::now();
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        if start.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            panic!("{name} never ended");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn is_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

/// Run `f` while a pump runs on a thread of its own (in a child process,
/// `in_child`).
fn with_pump(f: impl FnOnce()) {
    let (tx, rx) = mpsc::channel();
    let t = std::thread::spawn(move || {
        let p = Pump::new();
        tx.send(p.stopper()).unwrap();
        p.run();
    });
    let stopper = rx.recv().unwrap();
    f();
    stopper.stop();
    t.join().unwrap();
}

fn eventually(f: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn id_for(calls: &Calls, vk: u32) -> u32 {
    let start = Instant::now();
    loop {
        if let Some(c) = calls.lock().unwrap().iter().find(|c| c.0 == vk) {
            return c.2;
        }
        assert!(start.elapsed() < Duration::from_secs(3), "never registered");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_posted_hotkey_reaches_its_registrar_and_its_release_lets_go_of_the_key() {
    assert!(in_child("posted_hotkey", &[]).success());
}

#[test]
#[ignore = "run in a child by its test"]
fn posted_hotkey() {
    if is_child() {
        with_pump(posted_hotkey_steps);
    }
}

fn posted_hotkey_steps() {
    let calls = Calls::default();
    let mut reg = mac::registrar_with(Box::new(Recording(calls.clone())));
    reg.register(3, MOD_CTRL | MOD_ALT | MOD_NOREPEAT, 0x4D)
        .unwrap();
    let (vk, mods, id) = calls.lock().unwrap()[0];
    assert_eq!(
        (vk, mods),
        (0x4D, MOD_CTRL | MOD_ALT),
        "MOD_NOREPEAT dropped"
    );
    let keys = mac::Keys;
    assert!(mac::post_hotkey(id, true));
    assert_eq!(reg.wait(), Some(3));
    assert!(keys.is_down(0x4D), "down until its release");
    assert!(mac::post_hotkey(id, false));
    assert!(eventually(|| !keys.is_down(0x4D)));
    let stop = reg.stopper();
    std::thread::spawn(stop);
    assert_eq!(reg.wait(), None, "the stopper ends the wait");
}

#[test]
fn a_hotkey_released_at_once_is_a_tap_and_one_still_down_at_hold_is_a_hold() {
    assert!(in_child("tap_and_hold", &[]).success());
}

#[test]
#[ignore = "run in a child by its test"]
fn tap_and_hold() {
    if is_child() {
        with_pump(tap_and_hold_steps);
    }
}

fn tap_and_hold_steps() {
    let calls = Calls::default();
    let c = calls.clone();
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let mut hk = Hotkeys::start(
        Arc::new(move || mac::registrar_with(Box::new(Recording(c.clone())))),
        Arc::new(mac::Keys),
        vec![Resolved {
            action: Action::Restart,
            vk: 0x26,
            mods: MOD_CTRL | MOD_ALT,
        }],
        Arc::new(move |a, p| {
            let _ = tx.lock().unwrap().send((a, p));
        }),
    );
    assert_eq!(hk.registered().len(), 1);
    let id = id_for(&calls, 0x26);
    mac::post_hotkey(id, true);
    std::thread::sleep(Duration::from_millis(30));
    mac::post_hotkey(id, false);
    let got = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(got, (Action::Restart, Press::Tap));
    mac::post_hotkey(id, true);
    let got = rx.recv_timeout(HOLD * 4).unwrap();
    assert_eq!(
        got,
        (Action::Restart, Press::Hold),
        "heard while still held"
    );
    mac::post_hotkey(id, false);
    hk.stop();
}

#[test]
fn a_pump_stopped_from_another_thread_returns() {
    assert!(in_child("pump_stopped", &[]).success());
}

#[test]
#[ignore = "run in a child by its test"]
fn pump_stopped() {
    if is_child() {
        let start = Instant::now();
        with_pump(|| std::thread::sleep(Duration::from_millis(50)));
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
    }
}

#[test]
fn a_stop_before_the_pump_runs_is_not_lost() {
    assert!(in_child("stop_before_run", &[]).success());
}

#[test]
#[ignore = "run in a child by its test"]
fn stop_before_run() {
    if !is_child() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let t = std::thread::spawn(move || {
        let p = Pump::new();
        p.stopper().stop();
        p.run();
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    t.join().unwrap();
}

#[test]
fn registering_without_the_main_thread_pump_fails_instead_of_hanging() {
    // A test thread is not the main thread and no `run_main` pumps: the
    // real registrar refuses rather than waiting for a pump forever (and
    // registers nothing of the user's).
    let mut reg = (mac::platform().registrar)();
    let start = Instant::now();
    assert_eq!(
        reg.register(1, MOD_CTRL | MOD_ALT, 0x26),
        Err(mac::NO_EVENT_PUMP)
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn run_main_off_the_main_thread_just_runs_the_function() {
    assert_eq!(mac::run_main(|| 41 + 1), 42);
}

#[test]
fn windows_key_ids_translate_to_mac_key_codes() {
    let none = |_: char| None;
    assert_eq!(mac::key_code(0x26, &none), Some(0x7E), "up");
    assert_eq!(mac::key_code(0x28, &none), Some(0x7D), "down");
    assert_eq!(mac::key_code(0x25, &none), Some(0x7B), "left");
    assert_eq!(mac::key_code(0x27, &none), Some(0x7C), "right");
    assert_eq!(mac::key_code(0x4D, &none), Some(0x2E), "M by ANSI position");
    assert_eq!(mac::key_code(0x50, &none), Some(0x23), "P by ANSI position");
    assert_eq!(mac::key_code(0x31, &none), Some(0x12), "1");
    assert_eq!(mac::key_code(0xBE, &none), Some(0x2F), "period");
    assert_eq!(mac::key_code(0x13, &none), None, "unknown");
    // A letter is the key that types it on the layout (AZERTY: A is where
    // QWERTY has Q).
    let azerty = |c: char| (c == 'a').then_some(0x0C);
    assert_eq!(mac::key_code(0x41, &azerty), Some(0x0C));
    assert_eq!(
        mac::key_code(0x31, &azerty),
        Some(0x12),
        "digits by position"
    );
}

#[test]
fn modifiers_translate_to_carbon_bits() {
    assert_eq!(mac::carbon_modifiers(MOD_CTRL | MOD_ALT), 0x1000 | 0x0800);
    assert_eq!(mac::carbon_modifiers(MOD_SHIFT), 0x0200);
    assert_eq!(mac::carbon_modifiers(MOD_WIN | MOD_NOREPEAT), 0x0100);
    assert_eq!(mac::carbon_error(-9878), 1409, "another holder");
    assert_eq!(mac::carbon_error(-50), 50);
}

#[test]
fn macos_cannot_duck_and_reports_it() {
    let p = mac::platform();
    assert!(!p.duck);
    assert_eq!(p.name, "macos");
    let e = (p.audio)().sessions().err().unwrap();
    assert!(e.contains("not available"), "{e}");
    assert_eq!(p.layout.altgr_char(0x4D, false), None, "no AltGr");
}

#[test]
fn an_osascript_that_never_answers_times_out() {
    let start = Instant::now();
    let r = mac::osascript("delay 5", false, Duration::from_millis(300));
    assert!(r.as_ref().is_err_and(|e| timed_out(e)), "{r:?}");
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn an_osascript_that_answers_in_time_gives_its_output() {
    assert_eq!(
        mac::osascript("return \"ok\"", false, mac::CALL_TIMEOUT).as_deref(),
        Ok("ok")
    );
    assert_eq!(
        mac::osascript("1 + 1", true, mac::CALL_TIMEOUT).as_deref(),
        Ok("2")
    );
    assert!(mac::osascript("error \"no\"", false, mac::CALL_TIMEOUT).is_err());
}

const END_DIR: &str = "SONARA_SEAM_SESSION_END";

#[test]
fn sigterm_runs_the_session_end_callbacks_then_ends_the_process() {
    let dir = std::env::temp_dir().join(format!("sonara-mac-seam-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let ready = dir.join("ready");
    let watcher = std::thread::spawn(move || {
        assert!(eventually(|| ready.exists()), "the child is ready");
        let pid = std::fs::read_to_string(&ready).unwrap();
        // SAFETY: a signal to our own child.
        unsafe { libc_kill(pid.trim().parse().unwrap(), 15) };
    });
    let status = in_child("session_end_child", &[(END_DIR, dir.as_os_str())]);
    watcher.join().unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("ended")).unwrap(),
        "restored"
    );
    assert_eq!(status.code(), Some(143), "{status:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

/// The child of the SIGTERM test (does nothing unless it is that child).
#[test]
#[ignore = "run as a child by sigterm_runs_the_session_end_callbacks_then_ends_the_process"]
fn session_end_child() {
    let Some(dir) = std::env::var_os(END_DIR).map(std::path::PathBuf::from) else {
        return;
    };
    mac::set_exit_grace(Duration::from_millis(100));
    let ended = dir.join("ended");
    (mac::platform().session_end)(Box::new(move || {
        std::fs::write(&ended, "restored").unwrap();
    }));
    std::fs::write(dir.join("ready"), std::process::id().to_string()).unwrap();
    std::thread::sleep(Duration::from_secs(30));
}
