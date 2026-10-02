//! Sonara L3: the agent layer. Streamed text, turns, decisions spoken with
//! priority, earcons, three mute levels and optional summaries, on top of
//! L2 channels (one channel per agent session).
//!
//! `Agent` wraps a `sonara_channels::Channels` and uses only its public API
//! and the L1 facade's (`ReaderHandle::play_clip` for earcons). The rules
//! are pure (`rules::Rules`, see its module docs); this driver carries out
//! their actions under one lock, so messages apply in the order they
//! arrive:
//!
//! - `Speak` goes to the channel as an appended entry (whatever the
//!   channel's policy: a turn is many chunks); a decision also
//!   `prioritize`s the channel, so it is read before the other channels
//!   once the item playing ends.
//! - `Wipe` (a new turn, an answer) is `control(Stop, channel)`: the
//!   channel's unread entries are skipped and its item cut. For a new turn
//!   the reader is un-paused when the channel is the engaged one
//!   (`Channels::engaged`); a paused reader stays paused when another
//!   channel gets a new turn.
//! - `Silence` (muting) is `control(Stop)` over every channel.
//! - Earcons are played with `ReaderHandle::play_clip` and reported to
//!   `subscribe`rs.
//! - Timers and summarizer jobs run on their own threads, which hold the
//!   agent weakly and end with it.
pub mod decision;
pub mod earcon;
pub mod rules;
pub mod settings;
pub mod summarizer;

pub use decision::{AskKind, Choice};
pub use earcon::Earcon;
pub use rules::{Action, Ask, Job, Rules, Stale, Timer};
pub use settings::{Settings, Style, SummaryCommand, SummarySettings, Verbosity};
pub use sonara_channels::{Channels, Control, QueueMode};
pub use summarizer::Summarizer;

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Channels(#[from] sonara_channels::Error),
    #[error("{0}")]
    BadValue(String),
    #[error("this build has no summarizer")]
    NoSummarizer,
}

impl From<sonara_reader::Error> for Error {
    fn from(e: sonara_reader::Error) -> Self {
        Error::Channels(e.into())
    }
}

/// The summarizer this build offers: the real `claude -p` / `codex exec`
/// process with feature `summaries`, else none (summaries cannot be
/// turned on).
pub fn default_summarizer() -> Option<Arc<dyn Summarizer>> {
    #[cfg(feature = "summaries")]
    {
        Some(Arc::new(summarizer::ProcessSummarizer::default()))
    }
    #[cfg(not(feature = "summaries"))]
    {
        None
    }
}

/// How to build an `Agent`.
pub struct Config {
    pub settings: Settings,
    pub summarizer: Option<Arc<dyn Summarizer>>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            settings: Settings::default(),
            summarizer: default_summarizer(),
        }
    }
}

struct Inner {
    channels: Channels,
    rules: Mutex<Rules>,
    summarizer: Option<Arc<dyn Summarizer>>,
    subscribers: Mutex<Vec<Sender<Earcon>>>,
}

/// L3 over L2. Clones share it.
#[derive(Clone)]
pub struct Agent {
    inner: Arc<Inner>,
}

fn check(channel: &str) -> Result<()> {
    if channel.is_empty() {
        Err(sonara_channels::Error::EmptyChannel.into())
    } else {
        Ok(())
    }
}

impl Agent {
    pub fn new(channels: Channels, config: Config) -> Result<Agent> {
        if config.settings.summaries.enabled && config.summarizer.is_none() {
            return Err(Error::NoSummarizer);
        }
        Ok(Agent {
            inner: Arc::new(Inner {
                channels,
                rules: Mutex::new(Rules::new(config.settings)),
                summarizer: config.summarizer,
                subscribers: Mutex::new(Vec::new()),
            }),
        })
    }

    /// The channels underneath.
    pub fn channels(&self) -> &Channels {
        &self.inner.channels
    }

    fn lock(&self) -> MutexGuard<'_, Rules> {
        self.inner.lock()
    }

    /// Run `f` on the rules and carry out its actions. `Err(Stale)` from
    /// the rules is `Ok(false)`.
    fn apply(
        &self,
        channel: Option<&str>,
        f: impl FnOnce(&mut Rules) -> std::result::Result<Vec<Action>, Stale>,
    ) -> Result<bool> {
        if let Some(c) = channel {
            check(c)?;
        }
        let mut rules = self.lock();
        match f(&mut rules) {
            Ok(actions) => {
                self.inner.execute(&rules, actions)?;
                Ok(true)
            }
            Err(Stale) => Ok(false),
        }
    }

    /// Streamed prose. Returns false when it was dropped as late text of an
    /// earlier turn.
    pub fn stream(
        &self,
        channel: &str,
        turn: Option<&str>,
        delta: &str,
        index: u32,
        is_final: bool,
        t: Option<f64>,
    ) -> Result<bool> {
        self.apply(Some(channel), |r| {
            r.stream(channel, turn, delta, index, is_final, t)
        })
    }

    /// A new turn. Returns false when it is older than the channel's last
    /// one (dropped).
    pub fn turn_start(&self, channel: &str, turn: Option<&str>, t: Option<f64>) -> Result<bool> {
        self.apply(Some(channel), |r| r.turn_start(channel, turn, t))
    }

    /// The turn ended. Returns false when it was the end of an earlier turn
    /// (dropped, no earcon).
    pub fn turn_end(&self, channel: &str, turn: Option<&str>, t: Option<f64>) -> Result<bool> {
        self.apply(Some(channel), |r| r.turn_end(channel, turn, t))
    }

    /// A decision.
    pub fn ask(&self, channel: &str, ask: &Ask) -> Result<()> {
        self.apply(Some(channel), |r| Ok(r.ask(channel, ask)))
            .map(|_| ())
    }

    /// A tool runs.
    pub fn tool(&self, channel: &str, name: &str, summary: &str) -> Result<()> {
        self.apply(Some(channel), |r| Ok(r.tool(channel, name, summary)))
            .map(|_| ())
    }

    /// The user answered the question.
    pub fn answered(&self, channel: &str) -> Result<()> {
        self.apply(Some(channel), |r| Ok(r.answered(channel)))
            .map(|_| ())
    }

    /// Play an earcon (unless mute level 2).
    pub fn earcon(&self, e: Earcon) -> Result<()> {
        self.apply(None, |r| Ok(r.play(e))).map(|_| ())
    }

    /// Close a channel and forget its turn.
    pub fn close(&self, channel: &str) -> Result<()> {
        check(channel)?;
        let mut rules = self.lock();
        rules.close(channel);
        Ok(self.inner.channels.close(channel)?)
    }

    /// `control stop`: drop every channel's summary work and held
    /// decisions, then stop L2 (`control(Stop)` without a channel).
    pub fn stop(&self) -> Result<()> {
        let mut rules = self.lock();
        rules.stop_all();
        Ok(self.inner.channels.control(Control::Stop, None)?)
    }

    pub fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    pub fn awaiting(&self, channel: &str) -> bool {
        self.lock().awaiting(channel)
    }

    /// 0, 1 or 2. Muting also silences what is queued and playing.
    pub fn set_mute_level(&self, level: u8) -> Result<()> {
        if level > settings::MUTE_LEVEL_MAX {
            return Err(Error::BadValue(format!(
                "mute_level is 0 to {}",
                settings::MUTE_LEVEL_MAX
            )));
        }
        self.apply(None, |r| Ok(r.set_mute_level(level)))
            .map(|_| ())
    }

    pub fn set_verbosity(&self, v: Verbosity) {
        self.lock().settings.verbosity = v;
    }

    pub fn set_minqueue(&self, n: usize) -> Result<()> {
        if n > settings::MINQUEUE_MAX {
            return Err(Error::BadValue(format!(
                "minqueue is 0 to {}",
                settings::MINQUEUE_MAX
            )));
        }
        self.lock().settings.minqueue = n;
        Ok(())
    }

    /// Replace the summary settings. Turning summaries off does not cancel
    /// work already out.
    pub fn set_summaries(&self, s: SummarySettings) -> Result<()> {
        if s.enabled && self.inner.summarizer.is_none() {
            return Err(Error::NoSummarizer);
        }
        let (lo, hi) = settings::SUMMARY_TIMEOUT_S;
        if !(lo..=hi).contains(&s.timeout_s) {
            return Err(Error::BadValue(format!(
                "summaries.timeout is {lo} to {hi} seconds"
            )));
        }
        if s.settle_ms > settings::SUMMARY_SETTLE_MS_MAX {
            return Err(Error::BadValue(format!(
                "summaries.settle_ms is 0 to {}",
                settings::SUMMARY_SETTLE_MS_MAX
            )));
        }
        if s.model.trim().is_empty() {
            return Err(Error::BadValue("summaries.model is empty".into()));
        }
        self.lock().settings.summaries = s;
        Ok(())
    }

    /// Every earcon played from now on (dropping the receiver
    /// unsubscribes).
    pub fn subscribe(&self) -> Receiver<Earcon> {
        let (tx, rx) = channel();
        self.inner
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tx);
        rx
    }

    /// The pure rules' summary state of a channel (diagnostics, tests).
    pub fn summary_state(&self, channel: &str) -> Option<(bool, usize, usize, u32)> {
        self.lock().summary_state(channel)
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Rules> {
        // A panic under the lock already failed a test; keep serving.
        self.rules.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Carry out actions in order; the first error is returned after the
    /// rest were tried (a failed speak must not lose a later decision).
    fn execute(self: &Arc<Self>, rules: &Rules, actions: Vec<Action>) -> Result<()> {
        let mut first = Ok(());
        for a in actions {
            if let Err(e) = self.carry_out(rules, a) {
                if first.is_ok() {
                    first = Err(e);
                }
            }
        }
        first
    }

    fn carry_out(self: &Arc<Self>, rules: &Rules, a: Action) -> Result<()> {
        let ch = &self.channels;
        match a {
            Action::Speak {
                channel,
                text,
                decision,
            } => {
                ch.speak(&channel, &text, Some(QueueMode::Append), false, None)?;
                if decision {
                    ch.prioritize(&channel)?;
                }
            }
            Action::Earcon(e) => {
                let clip = e.clip();
                ch.reader()
                    .play_clip(clip.samples.clone(), clip.sample_rate)?;
                self.subscribers
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .retain(|s| s.send(e).is_ok());
            }
            Action::Wipe { channel, resume } => {
                if ch.channel(&channel).is_none() {
                    return Ok(());
                }
                let engaged = ch.engaged().as_deref() == Some(channel.as_str());
                ch.control(Control::Stop, Some(&channel))?;
                if resume && engaged {
                    let st = ch.reader().state()?;
                    if st.paused && st.now_playing.is_some() {
                        ch.control(Control::Play, None)?;
                    }
                }
            }
            Action::Silence => ch.control(Control::Stop, None)?,
            Action::Summarize(job) => self.summarize(rules, job),
            Action::Timer { after, timer } => {
                let weak = Arc::downgrade(self);
                let _ = std::thread::Builder::new()
                    .name("sonara-agent-timer".into())
                    .spawn(move || {
                        std::thread::sleep(after);
                        if let Some(inner) = weak.upgrade() {
                            inner.fire(&timer);
                        }
                    });
            }
        }
        Ok(())
    }

    fn fire(self: &Arc<Self>, timer: &Timer) {
        let mut rules = self.lock();
        let focused = self.channels.focused();
        let actions = rules.fire(timer, focused.as_deref());
        if let Err(e) = self.execute(&rules, actions) {
            eprintln!("[agent] timer {timer:?}: {e}");
        }
    }

    fn summarize(self: &Arc<Self>, rules: &Rules, job: Job) {
        let settings = rules.settings.summaries.clone();
        let summarizer = self.summarizer.clone();
        let weak: Weak<Inner> = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("sonara-summary".into())
            .spawn({
                let job = job.clone();
                move || {
                    let result = match &summarizer {
                        Some(s) => s.summarize(&job.text, &settings),
                        None => Err("no summarizer".into()),
                    };
                    let summary = match result {
                        Ok(s) => Some(s),
                        Err(reason) => {
                            eprintln!("[summary] {reason}");
                            None
                        }
                    };
                    if let Some(inner) = weak.upgrade() {
                        inner.digest_done(&job, summary);
                    }
                }
            });
        if spawned.is_err() {
            // No worker will answer: land it now as a failed summary, so
            // the raw text is still spoken and later summaries are not
            // parked behind it.
            let weak = Arc::downgrade(self);
            std::thread::spawn(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.digest_done(&job, None);
                }
            });
        }
    }

    fn digest_done(self: &Arc<Self>, job: &Job, summary: Option<String>) {
        let mut rules = self.lock();
        let actions = rules.digest_done(job, summary);
        if let Err(e) = self.execute(&rules, actions) {
            eprintln!("[agent] summary for {}: {e}", job.channel);
        }
    }
}
