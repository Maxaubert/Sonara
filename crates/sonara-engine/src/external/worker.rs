//! One blocking request on a short-lived thread, with a wait that `cancel`
//! ends at once (spec D9): `ureq` has no abort, and the reader needs a
//! cancelled synthesis to return promptly. The abandoned thread finishes on
//! its own (bounded by the request's timeouts) and its result is dropped.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How often a wait looks at the cancel generation.
const POLL: Duration = Duration::from_millis(10);

/// A cancel generation: `cancel` bumps it, and every wait that started
/// under an older generation ends with `Cancelled`.
#[derive(Debug, Default)]
pub struct CancelToken(AtomicU64);

/// The wait was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    pub fn generation(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn cancelled_since(&self, gen: u64) -> bool {
        self.generation() != gen
    }

    /// Run `f` on its own thread and wait for it, unless the generation
    /// moves past `gen` first.
    pub fn run<T: Send + 'static>(
        &self,
        gen: u64,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, Cancelled> {
        if self.cancelled_since(gen) {
            return Err(Cancelled);
        }
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("sonara-external-request".into())
            .spawn(move || {
                let _ = tx.send(f());
            });
        if spawned.is_err() {
            return Err(Cancelled);
        }
        loop {
            match rx.recv_timeout(POLL) {
                // A result that comes after a cancel is dropped too: a
                // program killed on the cancel answers with its failure,
                // which must not reach the fallback.
                Ok(_) if self.cancelled_since(gen) => return Err(Cancelled),
                Ok(v) => return Ok(v),
                Err(RecvTimeoutError::Timeout) if self.cancelled_since(gen) => {
                    return Err(Cancelled)
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(Cancelled),
            }
        }
    }

    /// Sleep `d`, ending early (with `Cancelled`) on a cancel.
    pub fn sleep(&self, gen: u64, d: Duration) -> Result<(), Cancelled> {
        let end = Instant::now() + d;
        loop {
            if self.cancelled_since(gen) {
                return Err(Cancelled);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            std::thread::sleep(left.min(POLL));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_result_comes_back() {
        let t = CancelToken::new();
        assert_eq!(t.run(t.generation(), || 7), Ok(7));
    }

    #[test]
    fn cancel_ends_the_wait_at_once() {
        let t = Arc::new(CancelToken::new());
        let gen = t.generation();
        let t2 = t.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            t2.cancel();
        });
        let start = Instant::now();
        let r = t.run(gen, || std::thread::sleep(Duration::from_secs(5)));
        assert_eq!(r, Err(Cancelled));
        assert!(start.elapsed() < Duration::from_millis(500));
        // A run under the new generation works.
        assert_eq!(t.run(t.generation(), || 1), Ok(1));
        assert_eq!(t.sleep(gen, Duration::from_secs(5)), Err(Cancelled));
        assert_eq!(t.sleep(t.generation(), Duration::from_millis(1)), Ok(()));
    }

    #[test]
    fn a_result_after_a_cancel_is_dropped() {
        let t = Arc::new(CancelToken::new());
        let gen = t.generation();
        let t2 = t.clone();
        // The work notices the cancel and answers at once (a killed program).
        let r = t.run(gen, move || {
            t2.cancel();
            "killed"
        });
        assert_eq!(r, Err(Cancelled));
    }
}
