//! Live checks of the Windows platform against this PC. They read (and one
//! briefly changes) the user's real audio sessions, media and hotkeys, so
//! they are `#[ignore]`: run them by hand with
//! `cargo test -p sonara-system --test win_live -- --ignored --nocapture`.
#![cfg(windows)]

use sonara_system::keymap::{self, Action};
use sonara_system::win;

#[test]
#[ignore = "touches this PC's audio sessions"]
fn lists_audio_sessions_of_every_render_device() {
    let sessions = (win::platform().audio)().sessions().expect("enumerate");
    for s in &sessions {
        println!("pid {} {:?} volume {:?}", s.pid(), s.name(), s.volume());
    }
}

#[test]
#[ignore = "touches this PC's media sessions"]
fn lists_media_sessions() {
    let sessions = (win::platform().media)().sessions().expect("GSMTC");
    for s in &sessions {
        println!("{:?} playing {:?}", s.app_id(), s.is_playing());
    }
}

#[test]
#[ignore = "registers this PC's global hotkeys for a moment"]
fn registers_and_releases_the_default_hotkeys() {
    let (bindings, _) = keymap::resolve(&keymap::defaults());
    let hk = sonara_system::Hotkeys::start(
        win::platform().registrar,
        bindings,
        std::sync::Arc::new(|a: Action| println!("pressed {a:?}")),
    );
    println!("registered {:?}", hk.registered());
    println!("collisions {:?}", hk.collisions());
    drop(hk);
}

#[test]
#[ignore = "reads this PC's keyboard layout"]
fn reports_altgr_conflicts_of_this_layout() {
    let (bindings, _) = keymap::resolve(&keymap::defaults());
    let found = keymap::altgr_conflicts(&bindings, win::platform().layout.as_ref());
    println!("{found:?}");
}
