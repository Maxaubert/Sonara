//! Health of one external engine (spec 8.1, 8.2): the circuit breaker for
//! transient failures, the blocked state for failures the user must fix,
//! and the once-per-episode cue. The clock is injected, so tests move time.
use super::error::ExtError;
use crate::Reason;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// The breaker opens for this long first, doubling up to `BREAKER_MAX`.
pub const BREAKER_FIRST: Duration = Duration::from_secs(30);
pub const BREAKER_MAX: Duration = Duration::from_secs(300);
/// A `quota` failure blocks this long, then one chunk probes.
pub const QUOTA_BLOCK: Duration = Duration::from_secs(600);

/// A failure the user must fix (or that waits for its time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub reason: Reason,
    pub message: String,
    /// `quota`: blocked until then.
    pub until: Option<Instant>,
    /// `bad_voice`: only this voice is blocked.
    pub voice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Transient {
    reason: Reason,
    message: String,
}

struct State {
    consecutive: u32,
    open_until: Option<Instant>,
    next_open: Duration,
    last_transient: Option<Transient>,
    blocked: Option<Blocked>,
    cue_said: bool,
    /// Something failed since the last success (a recovery is told).
    troubled: bool,
}

/// What `status` reports (spec 8.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Healthy,
    /// Breaker open or a quota block: it will try again by itself.
    Waiting {
        reason: Reason,
        message: String,
    },
    /// The user must change something.
    Blocked {
        reason: Reason,
        message: String,
    },
}

pub struct Health {
    clock: Clock,
    state: Mutex<State>,
}

impl Health {
    pub fn new(clock: Clock) -> Health {
        Health {
            clock,
            state: Mutex::new(State {
                consecutive: 0,
                open_until: None,
                next_open: BREAKER_FIRST,
                last_transient: None,
                blocked: None,
                cue_said: false,
                troubled: false,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn now(&self) -> Instant {
        (self.clock)()
    }

    /// Why a chunk for `voice` must not go to the network now, if so.
    pub fn skip(&self, voice: &str) -> Option<(Reason, String)> {
        let now = self.now();
        let mut s = self.lock();
        if let Some(b) = &s.blocked {
            let expired = b.until.is_some_and(|t| now >= t);
            let other_voice = b.voice.as_deref().is_some_and(|v| v != voice);
            if expired {
                // The probe: let this chunk through.
                s.blocked = None;
            } else if !other_voice {
                return Some((b.reason, b.message.clone()));
            }
        }
        match (s.open_until, &s.last_transient) {
            (Some(t), Some(tr)) if now < t => Some((tr.reason, tr.message.clone())),
            _ => None,
        }
    }

    /// Count a failure of a request for `voice`.
    pub fn record_failure(&self, e: &ExtError, voice: &str) {
        let now = self.now();
        let mut s = self.lock();
        s.troubled = true;
        if e.reason.is_transient() {
            s.consecutive += 1;
            s.last_transient = Some(Transient {
                reason: e.reason,
                message: e.message.clone(),
            });
            if s.consecutive >= 2 {
                let open = s.next_open;
                s.open_until = Some(now + open);
                s.next_open = (open * 2).min(BREAKER_MAX);
            }
            return;
        }
        s.blocked = Some(Blocked {
            reason: e.reason,
            message: e.message.clone(),
            until: (e.reason == Reason::Quota).then(|| now + QUOTA_BLOCK),
            voice: (e.reason == Reason::BadVoice).then(|| voice.to_string()),
        });
    }

    /// Block without a request (a missing key found before sending).
    pub fn block(&self, reason: Reason, message: String) {
        let mut s = self.lock();
        s.troubled = true;
        s.blocked = Some(Blocked {
            reason,
            message,
            until: None,
            voice: None,
        });
    }

    /// A request succeeded: close the breaker, clear any block, and allow
    /// a cue for the next episode. True when the engine was not healthy
    /// (a recovery worth a log line).
    pub fn record_success(&self) -> bool {
        let mut s = self.lock();
        let was = std::mem::replace(&mut s.troubled, false);
        s.consecutive = 0;
        s.open_until = None;
        s.next_open = BREAKER_FIRST;
        s.last_transient = None;
        s.blocked = None;
        s.cue_said = false;
        was
    }

    /// The user changed something (a key, the profile) or a test passed:
    /// forget every failure.
    pub fn clear(&self) {
        self.record_success();
    }

    /// Clear a `no_key` or `auth` block (a new key).
    pub fn clear_key_block(&self) {
        let mut s = self.lock();
        if s.blocked
            .as_ref()
            .is_some_and(|b| matches!(b.reason, Reason::NoKey | Reason::Auth))
        {
            s.blocked = None;
            s.cue_said = false;
        }
    }

    /// The blocked state now, if any.
    pub fn blocked(&self) -> Option<Blocked> {
        self.lock().blocked.clone()
    }

    /// True once per episode: the first fallback after a success (or a
    /// clear) speaks the cue, later ones do not.
    pub fn take_cue(&self) -> bool {
        let mut s = self.lock();
        !std::mem::replace(&mut s.cue_said, true)
    }

    pub fn view(&self) -> View {
        let now = self.now();
        let s = self.lock();
        if let Some(b) = &s.blocked {
            if b.until.is_none_or(|t| now < t) {
                return if b.reason == Reason::Quota {
                    View::Waiting {
                        reason: b.reason,
                        message: b.message.clone(),
                    }
                } else {
                    View::Blocked {
                        reason: b.reason,
                        message: b.message.clone(),
                    }
                };
            }
        }
        match (s.open_until, &s.last_transient) {
            (Some(t), Some(tr)) if now < t => View::Waiting {
                reason: tr.reason,
                message: tr.message.clone(),
            },
            _ => View::Healthy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestClock(Mutex<Instant>);

    fn rig() -> (Health, Arc<TestClock>) {
        let c = Arc::new(TestClock(Mutex::new(Instant::now())));
        let c2 = c.clone();
        (Health::new(Arc::new(move || *c2.0.lock().unwrap())), c)
    }

    impl TestClock {
        fn advance(&self, d: Duration) {
            *self.0.lock().unwrap() += d;
        }
    }

    fn fail(r: Reason) -> ExtError {
        ExtError::new(r, format!("{r} happened"))
    }

    #[test]
    fn two_transient_failures_open_the_breaker() {
        let (h, _) = rig();
        h.record_failure(&fail(Reason::Network), "v");
        assert_eq!(h.skip("v"), None, "one failure alone does not open it");
        assert_eq!(h.view(), View::Healthy);
        h.record_failure(&fail(Reason::Timeout), "v");
        assert_eq!(
            h.skip("v").map(|s| s.0),
            Some(Reason::Timeout),
            "two in a row open it"
        );
        assert!(matches!(
            h.view(),
            View::Waiting {
                reason: Reason::Timeout,
                ..
            }
        ));
    }

    #[test]
    fn backoff_doubles_to_300s() {
        let (h, clock) = rig();
        h.record_failure(&fail(Reason::Server), "v");
        let mut opened = Vec::new();
        for _ in 0..6 {
            h.record_failure(&fail(Reason::Server), "v");
            // Find how long it stays open.
            let mut secs = 0;
            while h.skip("v").is_some() {
                clock.advance(Duration::from_secs(1));
                secs += 1;
            }
            opened.push(secs);
            // The probe fails again.
        }
        assert_eq!(opened, vec![30, 60, 120, 240, 300, 300]);
    }

    #[test]
    fn probe_after_expiry_closes_on_success() {
        let (h, clock) = rig();
        h.record_failure(&fail(Reason::Network), "v");
        h.record_failure(&fail(Reason::Network), "v");
        assert!(h.skip("v").is_some());
        clock.advance(BREAKER_FIRST);
        assert_eq!(h.skip("v"), None, "expired: the next chunk probes");
        assert!(h.record_success(), "a recovery");
        h.record_failure(&fail(Reason::Network), "v");
        assert_eq!(h.skip("v"), None, "the backoff and the count were reset");
        h.record_failure(&fail(Reason::Network), "v");
        clock.advance(Duration::from_secs(29));
        assert!(h.skip("v").is_some(), "back to 30 s");
    }

    #[test]
    fn quota_blocks_ten_minutes() {
        let (h, clock) = rig();
        h.record_failure(&fail(Reason::Quota), "v");
        assert_eq!(h.skip("v").map(|s| s.0), Some(Reason::Quota));
        assert!(matches!(
            h.view(),
            View::Waiting {
                reason: Reason::Quota,
                ..
            }
        ));
        clock.advance(QUOTA_BLOCK - Duration::from_secs(1));
        assert!(h.skip("v").is_some());
        clock.advance(Duration::from_secs(1));
        assert_eq!(h.skip("v"), None, "one probe chunk after ten minutes");
    }

    #[test]
    fn auth_blocks_until_cleared() {
        let (h, clock) = rig();
        h.record_failure(&fail(Reason::Auth), "v");
        clock.advance(Duration::from_secs(3600));
        assert_eq!(h.skip("v").map(|s| s.0), Some(Reason::Auth));
        assert!(matches!(
            h.view(),
            View::Blocked {
                reason: Reason::Auth,
                ..
            }
        ));
        h.clear_key_block();
        assert_eq!(h.skip("v"), None);
        // A bad voice blocks that voice only.
        h.record_failure(&fail(Reason::BadVoice), "nova");
        assert!(h.skip("nova").is_some());
        assert_eq!(h.skip("alloy"), None);
        h.clear_key_block();
        assert!(h.skip("nova").is_some(), "a key does not fix a voice");
        h.clear();
        assert_eq!(h.skip("nova"), None);
    }

    #[test]
    fn cue_once_per_episode() {
        let (h, _) = rig();
        h.record_failure(&fail(Reason::Network), "v");
        assert!(h.take_cue());
        h.record_failure(&fail(Reason::Network), "v");
        assert!(!h.take_cue(), "same episode");
        assert!(!h.take_cue());
        h.record_success();
        h.record_failure(&fail(Reason::Server), "v");
        assert!(h.take_cue(), "a new episode after a success");
        h.clear();
        assert!(h.take_cue(), "and after a profile change");
    }
}
