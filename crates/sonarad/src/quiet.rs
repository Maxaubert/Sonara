//! "API requests are never sent if Sonara is muted or super-muted" (#227).
//!
//! Sonara is muted while the reader is muted (core `mute`, the mute hotkey
//! without `agent`) or the agent's `mute_level` is 1 or 2. Meanwhile the
//! external engines' `Hold` (`Engines::hold`) is held, so no request of any
//! kind reaches a provider: messages and their lookahead, "Session
//! changed" announcements and the spoken cues are read with the local
//! fallback (Kokoro, else OneCore), and voice lists are not fetched.
//!
//! - **Before** a change that mutes (`hold_now`), the hold is raised, which
//!   also cuts a request in flight; **after** any mute change (`sync`) it
//!   follows the truth (the reader's state and the agent's level), so a
//!   change that failed or unmuted releases it.
//! - The cues of a mute transition ("Muted.", "Super muted.", "Unmuted.")
//!   are always spoken locally (`Cues::speak_local`): "Unmuted." is said as
//!   the mute lifts, and is still part of the muted episode. Any other cue
//!   queued while muted is local too.
//! - The user's own actions still reach the provider: `engine_test` (the
//!   Test button) and a `preview` (`hold::explicit`).
use crate::agent_ext;
use sonara_engine::external::hold::Hold;
use sonara_reader::ReaderHandle;
use std::sync::{Arc, OnceLock};

/// Whether Sonara is muted, and the hold that keeps external engines quiet
/// meanwhile (none until the host allows external engines).
#[derive(Clone)]
pub struct Quiet {
    hold: Arc<OnceLock<Arc<Hold>>>,
    reader: ReaderHandle,
    agent: agent_ext::Slot,
}

impl Quiet {
    pub fn new(reader: ReaderHandle, agent: agent_ext::Slot) -> Quiet {
        Quiet {
            hold: Arc::new(OnceLock::new()),
            reader,
            agent,
        }
    }

    /// Follow `hold` from now on (`Server::with_engines`).
    pub fn attach(&self, hold: Arc<Hold>) {
        let _ = self.hold.set(hold);
        self.sync();
    }

    /// The reader is muted, or the agent's mute level is 1 or 2.
    pub fn muted(&self) -> bool {
        let reader = self.reader.state().is_ok_and(|s| s.muted);
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

    /// A change that mutes is about to apply: hold now, so nothing is sent
    /// from here on (a request in flight is cut).
    pub fn hold_now(&self) {
        if let Some(h) = self.hold.get() {
            h.set(true);
        }
    }

    /// After a mute change (or a failed one): hold exactly while muted.
    pub fn sync(&self) {
        if let Some(h) = self.hold.get() {
            h.set(self.muted());
        }
    }
}
