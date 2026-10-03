//! What happens to other apps' audio while speech plays: the audio mode
//! (`off`, `duck`, `pause`), the duck level, and the rules for engaging and
//! restoring, driven by L1 state events.
//!
//! - Engage (duck or pause) when an item is being read: playing, not paused
//!   and not muted.
//! - Restore at once on pause, mute, a mode change and when disarmed (no
//!   client needs the extension any more); restore after a short grace when
//!   the reader goes idle, so the gap between two messages does not bring
//!   the other apps up and down again.
//! - `shutdown` (the runtime exits) restores and waits for it; a crash is
//!   covered by the crash-restore files and the startup sweep (`recover`).
//!
//! Product rule: never leave other apps ducked or paused. All platform
//! work runs on one worker thread, where the platform objects are created
//! (COM).
//!
//! Every engage and restore that touched an app is reported as one line
//! (`AudioConfig::log`, #217): `media pause apps=<names> (reason: reading
//! item=<id> session=<label>)`, `media resume apps=<names> (reason: idle)`,
//! `duck apps=<names> level=<n> (reason: ...)`, `restore apps=<names>
//! (reason: ...)`, failures, and the startup sweep.
use crate::ducking::{app_names, Ducker};
use crate::log::{self as activity, LogFn};
use crate::pausing::MediaPauser;
use crate::platform::Platform;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Other apps' audio while Sonara speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMode {
    Off,
    Duck,
    Pause,
}

impl AudioMode {
    pub fn parse(s: &str) -> Option<AudioMode> {
        Some(match s {
            "off" => AudioMode::Off,
            "duck" => AudioMode::Duck,
            "pause" => AudioMode::Pause,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AudioMode::Off => "off",
            AudioMode::Duck => "duck",
            AudioMode::Pause => "pause",
        }
    }
}

/// The level other apps keep while ducked, in percent (the Python default).
pub const DEFAULT_DUCK_LEVEL: u8 = 30;
/// How long the reader may be idle before other apps come back.
pub const IDLE_GRACE: Duration = Duration::from_millis(400);

/// File names in the state folder.
pub const DUCK_STATE: &str = "duck_state.json";
pub const PAUSE_STATE: &str = "pause_state.json";

/// What the reader is doing, as far as other apps' audio is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Nothing to read.
    Idle,
    /// An item is current but silent (paused or muted).
    Held,
    /// An item is being read aloud.
    Speaking,
}

impl Activity {
    pub fn of(state: &sonara_reader::State) -> Activity {
        match &state.now_playing {
            None => Activity::Idle,
            Some(_) if state.paused || state.muted => Activity::Held,
            Some(_) => Activity::Speaking,
        }
    }
}

pub struct AudioConfig {
    /// Where the crash-restore files live (`<home>\state`).
    pub state_dir: PathBuf,
    /// Processes never ducked (this runtime's own pid).
    pub exclude_pids: Vec<u32>,
    pub idle_grace: Duration,
    /// Where the activity lines go (else stderr).
    pub log: Option<LogFn>,
}

impl AudioConfig {
    pub fn new(state_dir: PathBuf) -> Self {
        AudioConfig {
            state_dir,
            exclude_pids: vec![std::process::id()],
            idle_grace: IDLE_GRACE,
            log: None,
        }
    }

    /// Report the activity lines to `log`.
    pub fn with_log(mut self, log: Option<LogFn>) -> Self {
        self.log = log;
        self
    }
}

/// A snapshot for `get` and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub mode: AudioMode,
    pub duck_level: u8,
    pub armed: bool,
    pub activity: Activity,
    pub ducked: bool,
    pub paused: bool,
}

enum Cmd {
    /// The activity, with why when speaking (the item read, for the log).
    Activity(Activity, Option<String>),
    /// The mode in the status changed.
    Mode,
    Level(u8),
    /// The armed flag in the status changed.
    Arm,
    /// The startup crash sweep; acknowledged when done.
    Recover(Sender<()>),
    /// Acknowledged once every earlier command was carried out.
    Sync(Sender<()>),
    Shutdown(Sender<()>),
}

pub struct AudioControl {
    tx: Mutex<Option<Sender<Cmd>>>,
    status: Arc<Mutex<Status>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl AudioControl {
    /// Start the worker, disarmed, mode `off`.
    pub fn new(platform: &Platform, config: AudioConfig) -> AudioControl {
        let status = Arc::new(Mutex::new(Status {
            mode: AudioMode::Off,
            duck_level: DEFAULT_DUCK_LEVEL,
            armed: false,
            activity: Activity::Idle,
            ducked: false,
            paused: false,
        }));
        let (tx, rx) = mpsc::channel();
        let audio = platform.audio.clone();
        let media = platform.media.clone();
        let st = status.clone();
        let worker = std::thread::Builder::new()
            .name("sonara-system-audio".into())
            .spawn(move || {
                let w = Worker {
                    ducker: Ducker::new(audio(), config.state_dir.join(DUCK_STATE))
                        .with_log(config.log.clone()),
                    pauser: MediaPauser::new(media(), config.state_dir.join(PAUSE_STATE))
                        .with_log(config.log.clone()),
                    exclude: config.exclude_pids,
                    grace: config.idle_grace,
                    status: st,
                    idle_since: None,
                    log: config.log,
                    reason: None,
                };
                w.run(rx);
            })
            .expect("spawn the audio worker");
        AudioControl {
            tx: Mutex::new(Some(tx)),
            status,
            worker: Mutex::new(Some(worker)),
        }
    }

    fn send(&self, c: Cmd) -> bool {
        let tx = self.tx.lock().unwrap_or_else(|p| p.into_inner());
        tx.as_ref().map(|t| t.send(c).is_ok()).unwrap_or(false)
    }

    fn call(&self, make: impl FnOnce(Sender<()>) -> Cmd) {
        let (ack, done) = mpsc::channel();
        if self.send(make(ack)) {
            let _ = done.recv();
        }
    }

    pub fn status(&self) -> Status {
        *self.status.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn update(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap_or_else(|p| p.into_inner()));
    }

    /// Restore what a previous runtime left ducked or paused (the startup
    /// sweep). Waits until done.
    pub fn recover(&self) {
        self.call(Cmd::Recover);
    }

    pub fn set_activity(&self, a: Activity) {
        self.send(Cmd::Activity(a, None));
    }

    /// As `set_activity`, with why other apps would be engaged (the log
    /// line of an engage carries it).
    pub fn set_activity_because(&self, a: Activity, reason: Option<String>) {
        self.send(Cmd::Activity(a, reason));
    }

    pub fn set_mode(&self, mode: AudioMode) {
        self.update(|s| s.mode = mode);
        self.send(Cmd::Mode);
    }

    pub fn set_duck_level(&self, level: u8) {
        let level = level.min(100);
        self.update(|s| s.duck_level = level);
        self.send(Cmd::Level(level));
    }

    /// Armed: engage while speaking. Disarming restores at once.
    pub fn arm(&self, on: bool) {
        self.update(|s| s.armed = on);
        self.send(Cmd::Arm);
    }

    /// Wait until every earlier call was carried out.
    pub fn sync(&self) {
        self.call(Cmd::Sync);
    }

    /// Follow a reader's events: each `State` sets the activity. `initial`
    /// is the reader's state read after subscribing (a subscription sends
    /// none), so following mid-item engages at once. The thread ends with
    /// the event stream or this control.
    pub fn follow(
        self: &Arc<Self>,
        events: Receiver<sonara_reader::Event>,
        initial: Option<&sonara_reader::State>,
    ) {
        let me = Arc::downgrade(self);
        let first = initial.map(|s| (Activity::of(s), activity::reading_reason(s)));
        let _ = std::thread::Builder::new()
            .name("sonara-system-events".into())
            .spawn(move || {
                // Sent from this thread before any event, so a newer event
                // queued meanwhile still has the last word.
                let mut last = None;
                if let Some((a, why)) = first {
                    match me.upgrade() {
                        Some(c) => c.set_activity_because(a, why.clone()),
                        None => return,
                    }
                    last = Some((a, why));
                }
                while let Ok(e) = events.recv() {
                    let sonara_reader::Event::State(s) = e else {
                        continue;
                    };
                    // A new item read without a gap keeps the activity but
                    // changes the reason the next engage is logged with.
                    let now = (Activity::of(&s), activity::reading_reason(&s));
                    if last.as_ref() == Some(&now) {
                        continue;
                    }
                    let (a, why) = now.clone();
                    last = Some(now);
                    match me.upgrade() {
                        Some(c) => c.set_activity_because(a, why),
                        None => return,
                    }
                }
            });
    }

    /// Restore everything and stop the worker; later calls do nothing.
    pub fn shutdown(&self) {
        self.call(Cmd::Shutdown);
        self.tx.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(w) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = w.join();
        }
    }
}

impl Drop for AudioControl {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Worker {
    ducker: Ducker,
    pauser: MediaPauser,
    exclude: Vec<u32>,
    grace: Duration,
    status: Arc<Mutex<Status>>,
    /// When the reader went idle while engaged (the grace runs from here).
    idle_since: Option<Instant>,
    log: Option<LogFn>,
    /// Why the reader speaks (the item read), for the engage lines.
    reason: Option<String>,
}

impl Worker {
    fn snapshot(&self) -> Status {
        *self.status.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn publish(&self) {
        let mut s = self.status.lock().unwrap_or_else(|p| p.into_inner());
        s.ducked = self.ducker.is_ducked();
        s.paused = self.pauser.is_paused();
    }

    fn engaged(&self) -> bool {
        self.ducker.is_ducked() || self.pauser.is_paused()
    }

    fn note(&self, line: &str) {
        activity::emit(self.log.as_ref(), line);
    }

    fn engage_reason(&self) -> String {
        self.reason.clone().unwrap_or_else(|| "reading".into())
    }

    fn duck(&mut self, level: u8, why: &str) {
        self.ducker.duck(&self.exclude, level);
        let apps = app_names(&self.ducker.saved());
        if !apps.is_empty() {
            self.note(&duck_line(&apps, level, why));
        }
    }

    fn restore(&mut self, why: &str) {
        let apps = app_names(&self.ducker.saved());
        self.ducker.restore();
        if !apps.is_empty() {
            self.note(&format!(
                "restore apps={} (reason: {why})",
                activity::apps(&apps)
            ));
        }
    }

    fn pause(&mut self, why: &str) {
        self.pauser.pause();
        let apps = self.pauser.paused_apps().to_vec();
        if !apps.is_empty() {
            self.note(&format!(
                "media pause apps={} (reason: {why})",
                activity::apps(&apps)
            ));
        }
    }

    fn resume(&mut self, why: &str) {
        let apps = self.pauser.resume();
        if !apps.is_empty() {
            self.note(&format!(
                "media resume apps={} (reason: {why})",
                activity::apps(&apps)
            ));
        }
    }

    fn restore_all(&mut self, why: &str) {
        if self.ducker.is_ducked() {
            self.restore(why);
        }
        if self.pauser.is_paused() {
            self.resume(why);
        }
        self.idle_since = None;
    }

    /// Bring the platform in line with the status.
    fn reconcile(&mut self) {
        let s = self.snapshot();
        let want = s.armed && s.activity == Activity::Speaking && s.mode != AudioMode::Off;
        if want {
            self.idle_since = None;
            match s.mode {
                AudioMode::Duck => {
                    if self.pauser.is_paused() {
                        self.resume("mode duck");
                    }
                    if !self.ducker.is_ducked() {
                        let why = self.engage_reason();
                        self.duck(s.duck_level, &why);
                    }
                }
                AudioMode::Pause => {
                    if self.ducker.is_ducked() {
                        self.restore("mode pause");
                    }
                    if !self.pauser.is_paused() {
                        let why = self.engage_reason();
                        self.pause(&why);
                    }
                }
                AudioMode::Off => {}
            }
        } else if self.engaged() {
            let grace = s.armed
                && s.mode != AudioMode::Off
                && s.activity == Activity::Idle
                && !self.grace.is_zero();
            if !grace {
                self.restore_all(restore_reason(&s));
            } else {
                let since = *self.idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= self.grace {
                    self.restore_all("idle");
                }
            }
        } else {
            self.idle_since = None;
        }
        self.publish();
    }

    /// The startup sweep, with the worker's own platform objects (COM
    /// lives on this thread).
    fn recover(&mut self) {
        self.ducker.recover();
        self.pauser.recover();
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            let cmd = match self.idle_since {
                Some(since) => {
                    let left = self.grace.saturating_sub(since.elapsed());
                    match rx.recv_timeout(left) {
                        Ok(c) => Some(c),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => {
                            self.restore_all("shutdown");
                            return;
                        }
                    }
                }
                None => match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => {
                        self.restore_all("shutdown");
                        self.publish();
                        return;
                    }
                },
            };
            match cmd {
                None => {}
                Some(Cmd::Activity(a, why)) => {
                    if why.is_some() {
                        self.reason = why;
                    }
                    self.status
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .activity = a;
                }
                Some(Cmd::Mode) => {
                    // A switch never leaves the other backend engaged.
                    let mode = self.snapshot().mode;
                    self.restore_all(&format!("mode {}", mode.as_str()));
                }
                Some(Cmd::Level(level)) => {
                    if self.ducker.is_ducked() {
                        // Re-apply at the new level.
                        self.ducker.restore();
                        self.duck(level, "level change");
                    }
                }
                Some(Cmd::Arm) => {}
                Some(Cmd::Recover(ack)) => {
                    self.recover();
                    let _ = ack.send(());
                }
                Some(Cmd::Sync(ack)) => {
                    self.reconcile();
                    let _ = ack.send(());
                    continue;
                }
                Some(Cmd::Shutdown(ack)) => {
                    self.restore_all("shutdown");
                    self.publish();
                    let _ = ack.send(());
                    return;
                }
            }
            self.reconcile();
        }
    }
}

/// Why other apps come back at once (not after the idle grace).
fn restore_reason(s: &Status) -> &'static str {
    if !s.armed {
        "disarmed"
    } else if s.mode == AudioMode::Off {
        "mode off"
    } else if s.activity == Activity::Held {
        "paused or muted"
    } else {
        "idle"
    }
}

/// `duck apps=<names> level=<n> (reason: <why>)`.
pub fn duck_line(apps: &[String], level: u8, why: &str) -> String {
    format!(
        "duck apps={} level={level} (reason: {why})",
        activity::apps(apps)
    )
}
