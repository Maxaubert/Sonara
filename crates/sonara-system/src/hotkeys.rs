//! Global hotkeys: a pump thread registers the chords and waits for
//! presses (RegisterHotKey and the message loop must share one thread),
//! and a dispatch thread hands each press to the host. The pump never runs
//! the action itself, so a busy host can never stall hotkey capture (the
//! Python mute-hang). A rapid repeat of a toggle (pause, mute) is ignored.
//!
//! `stop` joins the pump thread, which unregisters every chord, before it
//! returns: a restart that re-registered while the old thread still held
//! the chords would lose them all (the Python H2 bug).
use crate::keymap::{Action, Resolved, MOD_NOREPEAT};
use crate::platform::Registrar;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// ERROR_HOTKEY_ALREADY_REGISTERED: another program owns the chord.
pub const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;

/// A repeat of the same toggle within this window is ignored.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// A chord that could not be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub action: Action,
    /// The OS error code.
    pub error: u32,
}

impl Collision {
    pub fn already_owned(&self) -> bool {
        self.error == ERROR_HOTKEY_ALREADY_REGISTERED
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

pub type Handler = Arc<dyn Fn(Action) + Send + Sync>;
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
    /// press on a dispatch thread. Returns once registration is done, so
    /// `collisions` is complete.
    pub fn start(factory: RegistrarFactory, bindings: Vec<Resolved>, handler: Handler) -> Hotkeys {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (press_tx, press_rx) = mpsc::channel::<Action>();
        let pump = std::thread::Builder::new()
            .name("sonara-hotkeys".into())
            .spawn(move || {
                let mut reg = factory();
                let mut ids = Vec::new();
                let mut collisions = Vec::new();
                let mut registered = Vec::new();
                for b in &bindings {
                    let id = b.action.id();
                    match reg.register(id, b.mods | MOD_NOREPEAT, b.vk) {
                        Ok(()) => {
                            ids.push(id);
                            registered.push(*b);
                        }
                        Err(error) => collisions.push(Collision {
                            action: b.action,
                            error,
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
                    if press_tx.send(action).is_err() {
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
                while let Ok(a) = press_rx.recv() {
                    handler(a);
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
