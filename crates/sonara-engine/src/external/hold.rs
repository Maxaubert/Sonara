//! Holding external engines back while the host is muted (#227: "API
//! requests are never sent if Sonara is muted or super-muted").
//!
//! - A `Hold` is shared by the host and every `External` built with it
//!   (`ExternalConfig::hold`). While it is held, an external engine sends no
//!   request of any kind: a chunk (the playing one or a lookahead one) is
//!   spoken with the local fallback, with no cue and no change of the
//!   engine's health, and a voice list is not fetched (the last one known
//!   is returned).
//! - Raising it (`set(true)`) ends every request in flight at once (the
//!   engines' cancel generation moves on), and a chunk whose request was
//!   cut is spoken with the fallback instead.
//! - Two scopes, per thread, override it for one call: `explicit` (an
//!   action the user asked for, the voice preview: it reaches the provider
//!   even while held) and `local` (Sonara's own mute cues such as
//!   "Unmuted.": the fallback speaks them even when not held).
//!   `External::test` (the Test button) is always explicit.
use super::worker::CancelToken;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// The host's mute, as the external engines see it.
#[derive(Debug, Default)]
pub struct Hold {
    held: AtomicBool,
    /// The cancel tokens of the engines built with this hold.
    tokens: Mutex<Vec<Weak<CancelToken>>>,
}

impl Hold {
    pub fn new() -> Hold {
        Hold::default()
    }

    pub fn is_held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }

    /// Hold or release; true when it changed. Holding ends every request in
    /// flight of the engines built with it.
    pub fn set(&self, held: bool) -> bool {
        let changed = self.held.swap(held, Ordering::SeqCst) != held;
        if changed && held {
            let mut tokens = self.tokens.lock().unwrap_or_else(|p| p.into_inner());
            tokens.retain(|t| match t.upgrade() {
                Some(t) => {
                    t.cancel();
                    true
                }
                None => false,
            });
        }
        changed
    }

    /// An engine's cancel token, ended when the hold is raised.
    pub(crate) fn watch(&self, token: &Arc<CancelToken>) {
        let mut tokens = self.tokens.lock().unwrap_or_else(|p| p.into_inner());
        tokens.retain(|t| t.strong_count() > 0);
        tokens.push(Arc::downgrade(token));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Normal,
    Explicit,
    Local,
}

thread_local! {
    static SCOPE: Cell<Scope> = const { Cell::new(Scope::Normal) };
}

pub(crate) fn scope() -> Scope {
    SCOPE.with(Cell::get)
}

fn within<T>(s: Scope, f: impl FnOnce() -> T) -> T {
    struct Restore(Scope);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCOPE.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(SCOPE.with(|c| c.replace(s)));
    f()
}

/// Run `f` as an action the user asked for (the voice preview): external
/// engines called on this thread meanwhile ignore the hold.
pub fn explicit<T>(f: impl FnOnce() -> T) -> T {
    within(Scope::Explicit, f)
}

/// Run `f` with external engines on this thread speaking with their local
/// fallback, as if held (Sonara's own mute cues).
pub fn local<T>(f: impl FnOnce() -> T) -> T {
    within(Scope::Local, f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_nest_and_restore() {
        assert_eq!(scope(), Scope::Normal);
        explicit(|| {
            assert_eq!(scope(), Scope::Explicit);
            local(|| assert_eq!(scope(), Scope::Local));
            assert_eq!(scope(), Scope::Explicit);
        });
        assert_eq!(scope(), Scope::Normal);
    }

    #[test]
    fn holding_cancels_the_watched_tokens_once() {
        let h = Hold::new();
        let t = Arc::new(CancelToken::new());
        h.watch(&t);
        assert!(h.set(true));
        assert_eq!(t.generation(), 1);
        assert!(!h.set(true));
        assert_eq!(t.generation(), 1, "no change, no cancel");
        assert!(h.set(false));
        assert_eq!(t.generation(), 1, "a release cancels nothing");
        drop(t);
        h.set(true);
        assert!(h.tokens.lock().unwrap().is_empty(), "gone engines pruned");
    }
}
