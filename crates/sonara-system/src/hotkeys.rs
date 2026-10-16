//! Global hotkeys: a pump thread registers the chords and waits for
//! presses (RegisterHotKey and the message loop must share one thread),
//! and a dispatch thread hands each press to the host. The pump never runs
//! the action itself, so a busy host can never stall hotkey capture (the
//! Python mute-hang). A rapid repeat of a toggle (pause, mute) is ignored.
//!
//! A chord arrives once however long it is held (MOD_NOREPEAT). For an
//! action that can be held (`Action::holdable`, #299: Up) the dispatch
//! thread then watches the key: let go before `HOLD` it is a tap, still
//! down at `HOLD` a hold, handed over at once (the user hears it while
//! still holding).
//!
//! `stop` joins the pump thread, which unregisters every chord, before it
//! returns: a restart that re-registered while the old thread still held
//! the chords would lose them all (the Python H2 bug).
use crate::keymap::{Action, Resolved, MOD_NOREPEAT};
use crate::platform::{KeyState, Registrar};
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// ERROR_HOTKEY_ALREADY_REGISTERED: another program owns the chord.
pub const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;

/// A repeat of the same toggle within this window is ignored.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// A holdable chord still down after this is a hold (#299).
pub const HOLD: Duration = Duration::from_millis(500);

/// How often the key is checked while a press may become a hold.
const HOLD_POLL: Duration = Duration::from_millis(10);

/// How a chord was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// Let go before `HOLD`, or an action that cannot be held.
    Tap,
    /// Still down at `HOLD` (`Action::holdable` only).
    Hold,
}

/// Watch `vk` after a press: `Hold` once it has been down for `HOLD`,
/// `Tap` as soon as it is up.
pub fn watch(keys: &dyn KeyState, vk: u32) -> Press {
    let start = Instant::now();
    while keys.is_down(vk) {
        if start.elapsed() >= HOLD {
            return Press::Hold;
        }
        std::thread::sleep(HOLD_POLL);
    }
    Press::Tap
}

/// A chord that could not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub action: Action,
    /// The OS error code.
    pub error: u32,
    /// Another of our actions bound to the same chord holds it (#43); the
    /// chord is not registered twice, `error` is then 1409.
    pub taken_by: Option<Action>,
}

impl Collision {
    /// Another program owns the chord.
    pub fn already_owned(&self) -> bool {
        self.error == ERROR_HOTKEY_ALREADY_REGISTERED && self.taken_by.is_none()
    }
}

/// Ignores a too-fast repeat of a debounced action.
#[derive(Debug, Default)]
pub struct Debounce {
    last: HashMap<Action, Instant>,
}

impl Debounce {
    /// True when the press should run.
    pub fn allow(&mut self, action: Action, now: Instant) -> bool {
        if !action.debounced() {
            return true;
        }
        if let Some(prev) = self.last.get(&action) {
            if now.saturating_duration_since(*prev) < DEBOUNCE {
                return false;
            }
        }
        self.last.insert(action, now);
        true
    }
}

pub type Handler = Arc<dyn Fn(Action, Press) + Send + Sync>;
pub type RegistrarFactory = Arc<dyn Fn() -> Box<dyn Registrar> + Send + Sync>;

/// Running hotkeys; `stop` (or dropping it) unregisters them.
pub struct Hotkeys {
    stopper: Option<Box<dyn Fn() + Send>>,
    pump: Option<JoinHandle<()>>,
    dispatch: Option<JoinHandle<()>>,
    collisions: Vec<Collision>,
    registered: Vec<Resolved>,
}

impl Hotkeys {
    /// Register `bindings` on a new pump thread and run `handler` for each
    /// press on a dispatch thread (`keys` tells a tap from a hold).
    /// Returns once registration is done, so `collisions` is complete.
    pub fn start(
        factory: RegistrarFactory,
        keys: Arc<dyn KeyState>,
        bindings: Vec<Resolved>,
        handler: Handler,
    ) -> Hotkeys {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (press_tx, press_rx) = mpsc::channel::<Resolved>();
        let pump = std::thread::Builder::new()
            .name("sonara-hotkeys".into())
            .spawn(move || {
                let mut reg = factory();
                let mut ids = Vec::new();
                let mut collisions = Vec::new();
                let mut registered = Vec::new();
                for b in &bindings {
                    let id = b.action.id();
                    // A chord an earlier action holds: Windows would refuse
                    // it with the same 1409 as another program's (#43).
                    let ours = registered
                        .iter()
                        .find(|r: &&Resolved| r.vk == b.vk && r.mods == b.mods);
                    if let Some(first) = ours {
                        collisions.push(Collision {
                            action: b.action,
                            error: ERROR_HOTKEY_ALREADY_REGISTERED,
                            taken_by: Some(first.action),
                        });
                        continue;
                    }
                    match reg.register(id, b.mods | MOD_NOREPEAT, b.vk) {
                        Ok(()) => {
                            ids.push(id);
                            registered.push(*b);
                        }
                        Err(error) => collisions.push(Collision {
                            action: b.action,
                            error,
                            taken_by: None,
                        }),
                    }
                }
                let _ = ready_tx.send((reg.stopper(), collisions, registered));
                let mut debounce = Debounce::default();
                while let Some(id) = reg.wait() {
                    let Some(action) = Action::from_id(id) else {
                        continue;
                    };
                    if !ids.contains(&id) || !debounce.allow(action, Instant::now()) {
                        continue;
                    }
                    let Some(b) = bindings.iter().find(|b| b.action == action) else {
                        continue;
                    };
                    if press_tx.send(*b).is_err() {
                        break;
                    }
                }
                for id in ids {
                    reg.unregister(id);
                }
            })
            .expect("spawn the hotkey thread");
        let dispatch = std::thread::Builder::new()
            .name("sonara-hotkey-actions".into())
            .spawn(move || {
                while let Ok(b) = press_rx.recv() {
                    let press = if b.action.holdable() {
                        watch(keys.as_ref(), b.vk)
                    } else {
                        Press::Tap
                    };
                    handler(b.action, press);
                }
            })
            .expect("spawn the hotkey action thread");
        let (stopper, collisions, registered) = match ready_rx.recv() {
            Ok(r) => (Some(r.0), r.1, r.2),
            // The pump thread died (a panicking registrar): nothing is
            // registered.
            Err(_) => (None, Vec::new(), Vec::new()),
        };
        Hotkeys {
            stopper,
            pump: Some(pump),
            dispatch: Some(dispatch),
            collisions,
            registered,
        }
    }

    pub fn collisions(&self) -> &[Collision] {
        &self.collisions
    }

    /// The bindings that were registered.
    pub fn registered(&self) -> &[Resolved] {
        &self.registered
    }

    /// Unregister every chord and wait for both threads.
    pub fn stop(&mut self) {
        if let Some(s) = self.stopper.take() {
            s();
        }
        if let Some(p) = self.pump.take() {
            let _ = p.join();
        }
        if let Some(d) = self.dispatch.take() {
            // The pump's end closes the press channel, so this ends once
            // the action running now is done.
            let _ = d.join();
        }
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        self.stop();
    }
}
