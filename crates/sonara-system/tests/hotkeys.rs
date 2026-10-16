//! Hotkey registration and dispatch with the fake registrar (ported rules
//! of the Python `tests/test_hotkeys*.py`: collisions recorded, not fatal;
//! a toggle double-tap ignored; a stop unregisters before a restart
//! registers again, H2).
mod common;

use common::eventually;
use sonara_system::fake::{Fake, FakeChord};
use sonara_system::hotkeys::{
    Debounce, Hotkeys, Press, DEBOUNCE, ERROR_HOTKEY_ALREADY_REGISTERED, HOLD,
};
use sonara_system::keymap::{self, Action, MOD_ALT, MOD_CTRL, MOD_NOREPEAT};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn start(fake: &Fake) -> (Hotkeys, Arc<Mutex<Vec<Action>>>) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    let (bindings, _) = keymap::resolve(&keymap::defaults());
    let p = fake.platform();
    let hk = Hotkeys::start(
        p.registrar,
        p.keys,
        bindings,
        Arc::new(move |a, _| g.lock().unwrap().push(a)),
    );
    (hk, got)
}

type Got = Arc<Mutex<Vec<(Action, Press)>>>;

fn start_pressed(fake: &Fake) -> (Hotkeys, Got) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let g = got.clone();
    let (bindings, _) = keymap::resolve(&keymap::defaults());
    let p = fake.platform();
    let hk = Hotkeys::start(
        p.registrar,
        p.keys,
        bindings,
        Arc::new(move |a, how| g.lock().unwrap().push((a, how))),
    );
    (hk, got)
}

const DOWN: u32 = 0x28;

#[test]
fn a_quick_press_of_down_is_a_tap() {
    // #299, #307, #309: a tap moves one section.
    let fake = Fake::new();
    let (_hk, got) = start_pressed(&fake);
    fake.hold_key(DOWN);
    fake.press(Action::Flush.id());
    std::thread::sleep(Duration::from_millis(60));
    fake.release_key(DOWN);
    assert!(eventually(|| got.lock().unwrap().len() == 1));
    assert_eq!(*got.lock().unwrap(), vec![(Action::Flush, Press::Tap)]);
}

#[test]
fn holding_down_is_a_hold_that_fires_before_the_release() {
    // #299, #307, #309: a hold acts while the key is still down.
    let fake = Fake::new();
    let (_hk, got) = start_pressed(&fake);
    fake.hold_key(DOWN);
    let t0 = Instant::now();
    fake.press(Action::Flush.id());
    assert!(eventually(|| got.lock().unwrap().len() == 1));
    assert!(t0.elapsed() >= HOLD);
    assert_eq!(*got.lock().unwrap(), vec![(Action::Flush, Press::Hold)]);
    fake.release_key(DOWN);
}

#[test]
fn mute_cannot_be_held() {
    let fake = Fake::new();
    let (_hk, got) = start_pressed(&fake);
    fake.hold_key(0x4D); // M
    let t0 = Instant::now();
    fake.press(Action::Mute.id());
    assert!(eventually(|| got.lock().unwrap().len() == 1));
    assert!(t0.elapsed() < HOLD, "no wait for a key that cannot be held");
    assert_eq!(*got.lock().unwrap(), vec![(Action::Mute, Press::Tap)]);
}

#[test]
fn the_defaults_are_registered_with_norepeat() {
    let fake = Fake::new();
    let (hk, _) = start(&fake);
    let w = fake.world();
    assert_eq!(w.hotkeys.len(), 4);
    let down = w.hotkeys.iter().find(|h| h.vk == DOWN).unwrap();
    assert_eq!(down.mods, MOD_CTRL | MOD_ALT | MOD_NOREPEAT);
    assert_eq!(down.id, Action::Flush.id());
    assert!(hk.collisions().is_empty());
    assert_eq!(hk.registered().len(), 4);
}

#[test]
fn a_press_dispatches_its_action() {
    let fake = Fake::new();
    let (_hk, got) = start(&fake);
    fake.press(Action::Flush.id());
    fake.press(Action::NextChannel.id());
    assert!(eventually(|| got.lock().unwrap().len() == 2));
    assert_eq!(
        *got.lock().unwrap(),
        vec![Action::Flush, Action::NextChannel]
    );
}

#[test]
fn a_press_of_an_unregistered_id_is_ignored() {
    let fake = Fake::new();
    let (_hk, got) = start(&fake);
    fake.press(Action::Faster.id()); // unbound by default
    fake.press(99);
    fake.press(Action::Flush.id());
    assert!(eventually(|| !got.lock().unwrap().is_empty()));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(*got.lock().unwrap(), vec![Action::Flush]);
}

#[test]
fn a_chord_another_program_owns_is_a_collision_not_a_failure() {
    let fake = Fake::new();
    fake.edit(|w| {
        w.taken.push(FakeChord {
            id: 0,
            mods: MOD_CTRL | MOD_ALT,
            vk: 0x4D,
        })
    });
    let (hk, got) = start(&fake);
    assert_eq!(hk.collisions().len(), 1);
    assert_eq!(hk.collisions()[0].action, Action::Mute);
    assert_eq!(hk.collisions()[0].error, ERROR_HOTKEY_ALREADY_REGISTERED);
    assert!(hk.collisions()[0].already_owned());
    assert_eq!(fake.world().hotkeys.len(), 3, "the other three still work");
    fake.press(Action::Flush.id());
    assert!(eventually(|| got.lock().unwrap().len() == 1));
}

#[test]
fn two_actions_on_one_chord_are_a_self_collision_not_another_program() {
    // #43: the second RegisterHotKey of a chord this thread holds fails
    // with 1409, which read as "another program owns it".
    let fake = Fake::new();
    let (mut bindings, _) = keymap::resolve(&keymap::defaults());
    bindings.push(keymap::Resolved {
        action: Action::Faster,
        vk: 0x4D,
        mods: MOD_CTRL | MOD_ALT,
    });
    let p = fake.platform();
    let hk = Hotkeys::start(p.registrar, p.keys, bindings, Arc::new(|_, _| {}));
    assert_eq!(hk.collisions().len(), 1);
    let c = &hk.collisions()[0];
    assert_eq!(c.taken_by, Some(Action::Mute));
    assert!(!c.already_owned());
    let mute = hk.collisions().iter().any(|c| c.action == Action::Mute);
    assert!(!mute, "the first action keeps the chord");
    assert_eq!(fake.world().hotkeys.len(), 4);
}

#[test]
fn stop_unregisters_so_a_restart_gets_every_chord_back() {
    // H2: a restart racing the old thread's unregister left every hotkey
    // dark. stop joins the pump, which unregisters first.
    let fake = Fake::new();
    let (mut hk, _) = start(&fake);
    hk.stop();
    assert!(fake.world().hotkeys.is_empty());
    let (hk2, got) = start(&fake);
    assert!(hk2.collisions().is_empty());
    assert_eq!(fake.world().hotkeys.len(), 4);
    fake.press(Action::Mute.id());
    assert!(eventually(|| got.lock().unwrap().len() == 1));
    drop(hk2);
    assert!(fake.world().hotkeys.is_empty(), "drop unregisters too");
}

#[test]
fn a_toggle_double_tap_is_ignored_directional_keys_are_not() {
    let mut d = Debounce::default();
    let t0 = Instant::now();
    assert!(d.allow(Action::Mute, t0));
    assert!(!d.allow(Action::Mute, t0 + Duration::from_millis(100)));
    assert!(d.allow(Action::Mute, t0 + DEBOUNCE + Duration::from_millis(1)));
    assert!(d.allow(Action::Pause, t0));
    assert!(d.allow(Action::Flush, t0));
    assert!(d.allow(Action::Flush, t0));
    assert!(d.allow(Action::NextChannel, t0));
    assert!(d.allow(Action::NextChannel, t0));
}

#[test]
fn a_slow_action_never_blocks_capture() {
    // The pump hands presses to the action thread and returns to waiting
    // (the Python mute-hang).
    let fake = Fake::new();
    let gate = Arc::new(Mutex::new(()));
    let held = gate.lock().unwrap();
    let got = Arc::new(Mutex::new(Vec::new()));
    let (g, gt) = (got.clone(), gate.clone());
    let (bindings, _) = keymap::resolve(&keymap::defaults());
    let p = fake.platform();
    let hk = Hotkeys::start(
        p.registrar,
        p.keys,
        bindings,
        Arc::new(move |a, _| {
            let _wait = gt.lock().unwrap();
            g.lock().unwrap().push(a);
        }),
    );
    fake.press(Action::Flush.id());
    fake.press(Action::Flush.id());
    fake.press(Action::NextChannel.id());
    std::thread::sleep(Duration::from_millis(50));
    drop(held);
    assert!(eventually(|| got.lock().unwrap().len() == 3));
    drop(hk);
}
