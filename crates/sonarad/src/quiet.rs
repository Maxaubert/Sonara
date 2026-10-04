//! "API requests are never sent if Sonara is muted or super-muted" (#227).
//!
//! Sonara is muted while the reader is muted (core `mute`, the mute hotkey
//! without `agent`) or the agent's `mute_level` is 1 or 2. Meanwhile the
//! external engines' `Hold` (`Engines::hold`) is held, so no request of any
//! kind reaches a provider: messages and their lookahead, "Session
//! changed" announcements and the spoken cues are read with the local
//! fallback (Kokoro, else OneCore), and voice lists are not fetched.
//!
//! - Every mute change runs inside `change()`, one at a time (a hotkey and
//!   a protocol unmute on two threads cannot leave a stale hold behind).
//!   **Before** a change that mutes (`Change::hold_now`), the hold is
//!   raised, which also cuts a request in flight; **after** it (the
//!   `Change` drops) the hold follows the truth (the reader's state and the
//!   agent's level), so a change that failed or unmuted releases it. A
//!   reader whose state cannot be read keeps the hold as it is.
//! - The cues of a mute transition ("Muted.", "Super muted.", "Unmuted.")
//!   are always spoken locally (`Cues::speak_local`): "Unmuted." is said as
//!   the mute lifts, and is still part of the muted episode. Any other cue
//!   queued while muted is local too.
//! - The user's own actions still reach the provider: `engine_test` (the
//!   Test button) and a `preview` (`hold::explicit`).
use crate::agent_ext;
use sonara_engine::external::hold::Hold;
use sonara_reader::ReaderHandle;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// Whether Sonara is muted, and the hold that keeps external engines quiet
/// meanwhile (none until the host allows external engines).
#[derive(Clone)]
pub struct Quiet {
    hold: Arc<OnceLock<Arc<Hold>>>,
    reader: ReaderHandle,
    agent: agent_ext::Slot,
    /// Serializes mute changes (`change`).
    changing: Arc<Mutex<()>>,
}

/// One mute change in progress (`Quiet::change`); on drop the hold follows
/// the outcome.
pub struct Change<'a> {
    quiet: &'a Quiet,
    _one_at_a_time: MutexGuard<'a, ()>,
}

impl Change<'_> {
    /// The change mutes: hold now, so nothing is sent from here on (a
    /// request in flight is cut).
    pub fn hold_now(&self) {
        if let Some(h) = self.quiet.hold.get() {
            h.set(true);
        }
    }
}

impl Drop for Change<'_> {
    fn drop(&mut self) {
        self.quiet.follow();
    }
}

impl Quiet {
    pub fn new(reader: ReaderHandle, agent: agent_ext::Slot) -> Quiet {
        Quiet {
            hold: Arc::new(OnceLock::new()),
            reader,
            agent,
            changing: Arc::new(Mutex::new(())),
        }
    }

    /// Follow `hold` from now on (`Server::with_engines`).
    pub fn attach(&self, hold: Arc<Hold>) {
        let _ = self.hold.set(hold);
        self.sync();
    }

    /// The reader is muted, or the agent's mute level is 1 or 2. A reader
    /// whose state cannot be read counts as it was (the current hold).
    pub fn muted(&self) -> bool {
        let reader = match self.reader.state() {
            Ok(s) => s.muted,
            Err(_) => self.is_held(),
        };
        reader
            || self
                .agent
                .get()
                .is_some_and(|a| a.settings().mute_level >= 1)
    }

    /// Whether external engines are held now.
    pub fn is_held(&self) -> bool {
        self.hold.get().is_some_and(|h| h.is_held())
    }

    /// Start a mute change: waits for any other one to finish.
    pub fn change(&self) -> Change<'_> {
        Change {
            quiet: self,
            _one_at_a_time: self.changing.lock().unwrap_or_else(|p| p.into_inner()),
        }
    }

    /// Hold exactly while muted (a mute level loaded at start).
    pub fn sync(&self) {
        drop(self.change());
    }

    fn follow(&self) {
        if let Some(h) = self.hold.get() {
            h.set(self.muted());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonara_audio::TestOutput;
    use sonara_engine::fake::FakeEngine;
    use sonara_reader::{Config, Control, Registry};
    use std::time::Duration;

    fn rig() -> (Quiet, ReaderHandle, Arc<Hold>) {
        let registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap();
        let q = Quiet::new(reader.clone(), Arc::new(OnceLock::new()));
        let hold = Arc::new(Hold::new());
        q.attach(hold.clone());
        (q, reader, hold)
    }

    #[test]
    fn a_stale_unmute_cannot_land_after_a_later_mute() {
        // A (an unmute on one connection) has read "not muted" when B (the
        // mute hotkey) starts: B waits for A, so A's release lands first
        // and B's mute is what stays.
        let (q, reader, hold) = rig();
        let a = q.change();
        let (q2, r2) = (q.clone(), reader.clone());
        let b = std::thread::spawn(move || {
            let c = q2.change();
            c.hold_now();
            r2.control(Control::Mute).unwrap();
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !b.is_finished(),
            "the mute waits for the change in progress"
        );
        assert!(!hold.is_held());
        drop(a);
        b.join().unwrap();
        assert!(reader.state().unwrap().muted);
        assert!(q.is_held(), "the later mute holds");
        reader.control(Control::Unmute).unwrap();
        q.sync();
        assert!(!q.is_held());
    }

    #[test]
    fn an_unreadable_reader_keeps_the_hold() {
        let (q, reader, _) = rig();
        reader.control(Control::Mute).unwrap();
        q.sync();
        assert!(q.is_held());
        reader.shutdown();
        assert!(reader.state().is_err());
        q.sync();
        assert!(q.is_held(), "not released because the state is unknown");
    }
}
