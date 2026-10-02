//! When the process ends (spec section 3, Lifetime).
//!
//! A client is a TCP connection that completed `hello`, or an open SSE event
//! stream; a plain HTTP request only counts as activity. The process exits
//! once it has had no client and nothing being read (a paused item does not
//! count) for the idle timeout (30 s by default), unless it runs standalone
//! or a client said `keep_alive: true` (sticky until the process ends). An
//! idle takeover or Ctrl+C end it at once.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub const DEFAULT_IDLE_EXIT: Duration = Duration::from_secs(30);

/// Why the process is ending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Idle,
    Takeover,
    Signal,
}

pub struct Lifetime {
    clients: AtomicUsize,
    keep_alive: AtomicBool,
    standalone: bool,
    idle_exit: Duration,
    last_active: Mutex<Instant>,
    exit: watch::Sender<Option<ExitReason>>,
}

/// Counts one client while it lives.
pub struct ClientGuard(Arc<Lifetime>);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Ordering::SeqCst);
        self.0.touch();
    }
}

impl Lifetime {
    pub fn new(idle_exit: Duration, standalone: bool) -> Arc<Self> {
        let (exit, _) = watch::channel(None);
        Arc::new(Lifetime {
            clients: AtomicUsize::new(0),
            keep_alive: AtomicBool::new(false),
            standalone,
            idle_exit,
            last_active: Mutex::new(Instant::now()),
            exit,
        })
    }

    pub fn client(self: &Arc<Self>) -> ClientGuard {
        self.clients.fetch_add(1, Ordering::SeqCst);
        self.touch();
        ClientGuard(self.clone())
    }

    pub fn clients(&self) -> usize {
        self.clients.load(Ordering::SeqCst)
    }

    pub fn set_keep_alive(&self) {
        self.keep_alive.store(true, Ordering::SeqCst);
    }

    /// Restart the idle countdown.
    pub fn touch(&self) {
        *self.last_active.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    /// True when the idle countdown may run at all.
    pub fn may_idle_exit(&self) -> bool {
        !self.standalone && !self.keep_alive.load(Ordering::SeqCst) && self.clients() == 0
    }

    /// True when the countdown ran out (call only when `may_idle_exit`).
    pub fn idle_expired(&self) -> bool {
        self.last_active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
            >= self.idle_exit
    }

    /// Ask the process to end; the first reason wins.
    pub fn request_exit(&self, why: ExitReason) {
        self.exit.send_if_modified(|r| {
            if r.is_none() {
                *r = Some(why);
                true
            } else {
                false
            }
        });
    }

    pub fn exit_requested(&self) -> Option<ExitReason> {
        *self.exit.borrow()
    }

    /// Resolves once an exit was requested.
    pub async fn wait_exit(&self) -> ExitReason {
        let mut rx = self.exit.subscribe();
        loop {
            if let Some(r) = *rx.borrow_and_update() {
                return r;
            }
            if rx.changed().await.is_err() {
                return ExitReason::Signal;
            }
        }
    }
}

/// Check the idle rule every `tick`. `busy` says whether the reader is
/// reading right now (that restarts the countdown). Once the countdown ran
/// out, `retire` makes the final decision atomically with the requests
/// that start speech (`Server::retire_if_idle`): the exit is requested only
/// when it says yes.
pub async fn monitor<F, G>(life: Arc<Lifetime>, tick: Duration, busy: F, retire: G)
where
    F: Fn() -> bool + Send + Sync + Clone + 'static,
    G: Fn() -> bool + Send + Sync + Clone + 'static,
{
    let mut interval = tokio::time::interval(tick);
    loop {
        interval.tick().await;
        if life.exit_requested().is_some() {
            return;
        }
        if !life.may_idle_exit() {
            life.touch();
            continue;
        }
        let b = busy.clone();
        if tokio::task::spawn_blocking(b).await.unwrap_or(false) {
            life.touch();
            continue;
        }
        if life.idle_expired() {
            let r = retire.clone();
            if tokio::task::spawn_blocking(r).await.unwrap_or(false) {
                life.request_exit(ExitReason::Idle);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_monitor_exits_only_when_retire_agrees() {
        let life = Lifetime::new(Duration::ZERO, false);
        let agree = Arc::new(AtomicBool::new(false));
        let a = agree.clone();
        let task = tokio::spawn(monitor(
            life.clone(),
            Duration::from_millis(10),
            || false,
            move || a.load(Ordering::SeqCst),
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(life.exit_requested(), None, "retire said no");
        agree.store(true, Ordering::SeqCst);
        let why = tokio::time::timeout(Duration::from_secs(5), life.wait_exit())
            .await
            .unwrap();
        assert_eq!(why, ExitReason::Idle);
        task.await.unwrap();
    }

    #[test]
    fn a_client_or_keep_alive_blocks_the_idle_exit() {
        let life = Lifetime::new(Duration::ZERO, false);
        assert!(life.may_idle_exit());
        let guard = life.client();
        assert!(!life.may_idle_exit());
        drop(guard);
        assert!(life.may_idle_exit());
        life.set_keep_alive();
        assert!(!life.may_idle_exit());
    }

    #[test]
    fn standalone_never_idles_out() {
        let life = Lifetime::new(Duration::ZERO, true);
        assert!(!life.may_idle_exit());
    }

    #[test]
    fn the_first_exit_reason_wins() {
        let life = Lifetime::new(DEFAULT_IDLE_EXIT, false);
        life.request_exit(ExitReason::Takeover);
        life.request_exit(ExitReason::Idle);
        assert_eq!(life.exit_requested(), Some(ExitReason::Takeover));
    }

    #[tokio::test]
    async fn the_monitor_exits_once_idle_and_waits_while_busy() {
        let life = Lifetime::new(Duration::from_millis(50), false);
        let busy = Arc::new(AtomicBool::new(true));
        let b = busy.clone();
        let task = tokio::spawn(monitor(
            life.clone(),
            Duration::from_millis(10),
            move || b.load(Ordering::SeqCst),
            || true,
        ));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(life.exit_requested(), None, "busy keeps it alive");
        busy.store(false, Ordering::SeqCst);
        let why = tokio::time::timeout(Duration::from_secs(5), life.wait_exit())
            .await
            .unwrap();
        assert_eq!(why, ExitReason::Idle);
        task.await.unwrap();
    }
}
