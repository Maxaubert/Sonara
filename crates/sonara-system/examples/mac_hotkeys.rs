//! A live probe of the macOS hotkeys, run by `tests/mac_live.rs` (or by
//! hand): the main thread runs the Carbon event pump (`mac::run_main`), as
//! in sonarad, and the default hotkeys are registered on it.
//!
//! `mac_hotkeys register` registers the defaults, prints what was
//! registered and what collided, and releases them. `mac_hotkeys listen
//! <seconds>` keeps them for that long and prints every press with its tap
//! or hold. Nothing else of the user's is touched.
fn main() {
    #[cfg(target_os = "macos")]
    probe::run();
    #[cfg(not(target_os = "macos"))]
    eprintln!("mac_hotkeys: macOS only");
}

#[cfg(target_os = "macos")]
mod probe {
    use sonara_system::keymap::{self, Action};
    use sonara_system::{mac, Hotkeys, Press};
    use std::sync::Arc;
    use std::time::Duration;

    pub fn run() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let listen = match args.first().map(String::as_str) {
            Some("listen") => args
                .get(1)
                .and_then(|s| s.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(Duration::from_secs(15)),
            _ => Duration::ZERO,
        };
        mac::run_main(move || {
            let (bindings, _) = keymap::resolve(&keymap::defaults());
            let p = mac::platform();
            let mut hk = Hotkeys::start(
                p.registrar,
                p.keys,
                bindings,
                Arc::new(|a: Action, how: Press| println!("pressed {} {how:?}", a.as_str())),
            );
            let labels: Vec<String> = hk
                .registered()
                .iter()
                .map(|r| keymap::combo_label(r.mods, r.vk))
                .collect();
            println!("registered {} {}", labels.len(), labels.join(", "));
            for c in hk.collisions() {
                println!("collision {} error {}", c.action.as_str(), c.error);
            }
            if !listen.is_zero() {
                println!(
                    "press {} (tap, then hold) within {} s",
                    labels.first().map(String::as_str).unwrap_or("a hotkey"),
                    listen.as_secs()
                );
                std::thread::sleep(listen);
            }
            hk.stop();
            println!("released");
        });
    }
}
