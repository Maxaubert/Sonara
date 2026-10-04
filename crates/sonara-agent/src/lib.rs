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
//! - `Speak` goes to the channel as an appended entry (`Channels::add`,
//!   whatever the channel's policy: a turn is many chunks); a decision also
//!   `prioritize`s the channel, so it is read before the other channels
//!   once the item playing ends; a summary delivery `authorize`s it past
//!   the background policy.
//! - **Background policy** (`set_background_policy`): `earcon_only` turns
//!   on L2's focus-only gate, so only the focused channel (the session the
//!   user last prompted) is read; `all` turns it off.
//! - **Per-channel mute** (`set_channel_muted`): L2 holds the channel's
//!   text unread and never switches to it (earcons still play).
//! - **Dead sessions**: a channel with no message for `Config::forget_after`
//!   has its turn state freed (as `close`) and, when it is neither focused
//!   nor being read and has nothing unread, its L2 channel closed: a
//!   session that died without `SessionEnd` does not keep memory for the
//!   life of the runtime (the Python `forget_session`). `forget` does it
//!   at once.
//! - `Wipe` (a new turn, an answer) is `control(Stop, channel)`: the
//!   channel's unread entries are skipped and its item cut. For a new turn
//!   the reader is un-paused when the channel is the engaged one
//!   (`Channels::engaged`); a paused reader stays paused when another
//!   channel gets a new turn.
//! - `Silence` (muting) is `control(Stop)` over every channel.
//! - `flush` (the flush hotkey, #228) stops only the session being read:
//!   L2 `stop_reading`, then `Rules::flush` on that channel. The other
//!   sessions are untouched.
//! - Earcons are played with `ReaderHandle::play_clip` and reported to
//!   `subscribe`rs. The clips come from `Config::earcons` (the bundled
//!   ones, or a folder of custom WAVs in front: `earcon::Library`).
//! - **Session switches** (the Python daemon's "Session changed"): the
//!   agent sets L2's announcement texts to `SESSION_CHANGED` /
//!   `SESSION_CHANGED_AGAIN` and plays the `session_change` earcon right
//!   before each announcement is handed to the reader (L2's
//!   `on_announce`), so every switch the user hears, automatic or manual,
//!   chimes first and then says "Session changed: <label>.". Not at mute
//!   level 2.
//! - Timers and summarizer jobs run on their own threads, which hold the
//!   agent weakly and end with it.
//! - **Trace** (#219, `on_trace`): every message's outcome for the
//!   troubleshooting log: text added to a channel (with its L2 entry, its
//!   kind and why it may wait), what the rules did not speak and why
//!   (`rules::Note`), late text dropped, earcons and wipes (L2 reports the
//!   entries a wipe drops, with the reason given here: `turn_start`,
//!   `answered`, `mute`, `stop`, `flush`). The hook runs under the agent's lock and
//!   must not call back into the agent.
pub mod decision;
pub mod earcon;
pub mod rules;
pub mod settings;
pub mod summarizer;

pub use decision::{AskKind, Choice};
pub use earcon::{Earcon, Library};
pub use rules::{Action, Ask, Job, Note, Rules, Stale, Timer};
pub use settings::{
    BackgroundPolicy, ReadMode, Settings, Style, SummaryCommand, SummarySettings, Verbosity,
};
pub use sonara_channels::{Channels, Control, Flushed, QueueMode};
pub use summarizer::Summarizer;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

/// How long a channel may stay silent before its turn state is freed
/// (`Config::forget_after`): far longer than any turn, so only a session
/// that died without `SessionEnd` is affected.
pub const FORGET_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

/// The switch announcement (`{label}`: the session's label), as the Python
/// plugin said it.
pub const SESSION_CHANGED: &str = "Session changed: {label}.";
/// The switch announcement when the session is read again from the top.
pub const SESSION_CHANGED_AGAIN: &str = "Session changed: {label}, reading again.";

/// How often the dead-session sweep runs at most.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

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

/// What the agent did, for the troubleshooting log (`Agent::on_trace`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    /// What caused it: the message (`stream`, `turn_start`, `turn_end`,
    /// `ask question`, `ask permission`, `ask plan`, `tool`, `answered`,
    /// `earcon`, `mute_level`, `stop`, `flush`), a `timer` or a `summary`
    /// landing.
    pub source: String,
    pub channel: Option<String>,
    pub what: Traced,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Traced {
    /// `text` was added to the channel as L2 entry `entry`; `waits` says
    /// why it may not be read soon (a muted channel, the background
    /// policy).
    Spoken {
        kind: &'static str,
        entry: u64,
        text: String,
        decision: bool,
        waits: Option<&'static str>,
    },
    /// Something the rules did not speak now, and why.
    Note(Note),
    /// The message was late text of an earlier turn and was dropped
    /// (stamped before the channel's last `turn_start`, #174).
    Late,
    Earcon(Earcon),
    /// The channel's unread text (every channel's when `channel` is
    /// `None`) was dropped and its item cut, for `reason`.
    Wiped {
        reason: &'static str,
    },
}

/// The hook of `Agent::on_trace`.
pub type TraceHook = Arc<dyn Fn(&Trace) + Send + Sync>;

/// How to build an `Agent`.
pub struct Config {
    pub settings: Settings,
    pub summarizer: Option<Arc<dyn Summarizer>>,
    /// A channel silent this long has its turn state freed (module docs).
    pub forget_after: Duration,
    /// The earcon clips (default: the bundled ones).
    pub earcons: Arc<Library>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            settings: Settings::default(),
            summarizer: default_summarizer(),
            forget_after: FORGET_AFTER,
            earcons: Arc::new(Library::bundled()),
        }
    }
}

/// Earcons as they play (`Agent::subscribe`). Dropping it unsubscribes:
/// the agent forgets its sender at the next earcon or subscription.
pub struct EarconStream {
    rx: Receiver<Earcon>,
    _alive: Arc<()>,
}

impl std::ops::Deref for EarconStream {
    type Target = Receiver<Earcon>;

    fn deref(&self) -> &Receiver<Earcon> {
        &self.rx
    }
}

struct Subscriber {
    tx: Sender<Earcon>,
    alive: Weak<()>,
}

/// When each channel last had a message, for the dead-session sweep.
struct Seen {
    at: HashMap<String, Instant>,
    swept: Instant,
}

struct Inner {
    channels: Channels,
    rules: Mutex<Rules>,
    summarizer: Option<Arc<dyn Summarizer>>,
    subscribers: Mutex<Vec<Subscriber>>,
    forget_after: Duration,
    /// Locked only while `rules` is held.
    seen: Mutex<Seen>,
    earcons: Arc<Library>,
    /// The mute level, readable without the rules' lock (the
    /// session-change earcon is played under L2's lock).
    mute_level: AtomicU8,
    /// `Agent::on_trace`.
    trace: Mutex<Option<TraceHook>>,
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
        channels.set_focus_only(config.settings.background == BackgroundPolicy::EarconOnly)?;
        channels.set_announce_texts(SESSION_CHANGED, SESSION_CHANGED_AGAIN);
        let mute_level = AtomicU8::new(config.settings.mute_level);
        let inner = Arc::new(Inner {
            channels: channels.clone(),
            rules: Mutex::new(Rules::new(config.settings)),
            summarizer: config.summarizer,
            subscribers: Mutex::new(Vec::new()),
            forget_after: config.forget_after,
            seen: Mutex::new(Seen {
                at: HashMap::new(),
                swept: Instant::now(),
            }),
            earcons: config.earcons,
            mute_level,
            trace: Mutex::new(None),
        });
        // Called under L2's lock: it only plays a clip and reports it.
        let weak = Arc::downgrade(&inner);
        channels.on_announce(Some(Arc::new(move |_| {
            if let Some(inner) = weak.upgrade() {
                if inner.mute_level.load(Ordering::SeqCst) < 2 {
                    if let Err(e) = inner.play(Earcon::SessionChange) {
                        eprintln!("[agent] session_change earcon: {e}");
                    }
                }
            }
        })));
        Ok(Agent { inner })
    }

    /// Report what the agent does to `hook` (`None` removes it): module
    /// docs. It runs under the agent's lock.
    pub fn on_trace(&self, hook: Option<TraceHook>) {
        *self.inner.trace.lock().unwrap_or_else(|p| p.into_inner()) = hook;
    }

    /// The earcon clips in force (bundled, or custom files in front).
    pub fn earcons(&self) -> &Arc<Library> {
        &self.inner.earcons
    }

    /// The channels underneath.
    pub fn channels(&self) -> &Channels {
        &self.inner.channels
    }

    fn lock(&self) -> MutexGuard<'_, Rules> {
        self.inner.lock()
    }

    /// Run `f` on the rules and carry out its actions; `source` names the
    /// message for the trace. `Err(Stale)` from the rules is `Ok(false)`.
    fn apply(
        &self,
        source: &str,
        channel: Option<&str>,
        f: impl FnOnce(&mut Rules) -> std::result::Result<Vec<Action>, Stale>,
    ) -> Result<bool> {
        if let Some(c) = channel {
            check(c)?;
        }
        let mut rules = self.lock();
        self.inner.seen(&mut rules, channel);
        match f(&mut rules) {
            Ok(actions) => {
                self.inner.execute(&rules, source, channel, actions)?;
                Ok(true)
            }
            Err(Stale) => {
                self.inner.trace(source, channel, Traced::Late);
                Ok(false)
            }
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
        self.apply("stream", Some(channel), |r| {
            r.stream(channel, turn, delta, index, is_final, t)
        })
    }

    /// A new turn. Returns false when it is older than the channel's last
    /// one (dropped).
    pub fn turn_start(&self, channel: &str, turn: Option<&str>, t: Option<f64>) -> Result<bool> {
        self.apply("turn_start", Some(channel), |r| {
            r.turn_start(channel, turn, t)
        })
    }

    /// The turn ended. Returns false when it was the end of an earlier turn
    /// (dropped, no earcon).
    pub fn turn_end(&self, channel: &str, turn: Option<&str>, t: Option<f64>) -> Result<bool> {
        self.apply("turn_end", Some(channel), |r| r.turn_end(channel, turn, t))
    }

    /// A decision.
    pub fn ask(&self, channel: &str, ask: &Ask) -> Result<()> {
        let source = format!("ask {}", ask.kind.as_str());
        self.apply(&source, Some(channel), |r| Ok(r.ask(channel, ask)))
            .map(|_| ())
    }

    /// A tool runs.
    pub fn tool(&self, channel: &str, name: &str, summary: &str) -> Result<()> {
        self.apply(
            "tool",
            Some(channel),
            |r| Ok(r.tool(channel, name, summary)),
        )
        .map(|_| ())
    }

    /// The user answered the question.
    pub fn answered(&self, channel: &str) -> Result<()> {
        self.apply("answered", Some(channel), |r| Ok(r.answered(channel)))
            .map(|_| ())
    }

    /// Play an earcon (unless mute level 2).
    pub fn earcon(&self, e: Earcon) -> Result<()> {
        self.apply("earcon", None, |r| Ok(r.play(e))).map(|_| ())
    }

    /// Close a channel and forget its turn.
    pub fn close(&self, channel: &str) -> Result<()> {
        check(channel)?;
        let mut rules = self.lock();
        rules.close(channel);
        self.inner.forget_seen(channel);
        Ok(self.inner.channels.close(channel)?)
    }

    /// Forget a channel at once (a session that died without
    /// `SessionEnd`): its turn state is freed and its L2 channel closed if
    /// it is open. Unlike `close`, an unknown channel is not an error.
    pub fn forget(&self, channel: &str) -> Result<()> {
        check(channel)?;
        let mut rules = self.lock();
        rules.close(channel);
        self.inner.forget_seen(channel);
        if self.inner.channels.channel(channel).is_some() {
            self.inner.channels.close(channel)?;
        }
        Ok(())
    }

    /// The channels with turn state (diagnostics, tests).
    pub fn tracked(&self) -> Vec<String> {
        self.lock().channels()
    }

    /// Mute or unmute one channel's speech (its earcons still play): its
    /// text waits unread and a channel switch never lands on it (L2
    /// `set_muted`).
    pub fn set_channel_muted(&self, channel: &str, muted: bool) -> Result<()> {
        check(channel)?;
        Ok(self.inner.channels.set_muted(channel, muted)?)
    }

    /// `all` reads every channel; `earcon_only` only the focused one.
    pub fn set_background_policy(&self, policy: BackgroundPolicy) -> Result<()> {
        let mut rules = self.lock();
        rules.settings.background = policy;
        Ok(self
            .inner
            .channels
            .set_focus_only(policy == BackgroundPolicy::EarconOnly)?)
    }

    /// `control stop`: drop every channel's summary work and held
    /// decisions, then stop L2 (`control(Stop)` without a channel).
    /// Every drop is noted for the troubleshooting log (#228).
    pub fn stop(&self) -> Result<()> {
        let mut rules = self.lock();
        rules.stop_all();
        self.inner
            .trace("stop", None, Traced::Wiped { reason: "stop" });
        self.inner.execute(&rules, "stop", None, Vec::new())?;
        Ok(self
            .inner
            .channels
            .control_because(Control::Stop, None, "stop")?)
    }

    /// The flush hotkey (#228): stop only the session being read. Its
    /// item, its unread text, its held prose, the prose kept for its
    /// summary and its summary work are dropped (`Rules::flush`, L2
    /// `stop_reading`); every other session keeps its text, summaries and
    /// turns still arriving, and is read next as usual. Text spoken to the
    /// reader directly is skipped one item at a time; idle, nothing
    /// happens. Mute is the way to silence everything.
    pub fn flush(&self) -> Result<Flushed> {
        let mut rules = self.lock();
        let flushed = self.inner.channels.stop_reading("flush")?;
        if let Flushed::Channel(ch) = &flushed {
            self.inner
                .trace("flush", Some(ch), Traced::Wiped { reason: "flush" });
            rules.flush(ch);
            self.inner.execute(&rules, "flush", Some(ch), Vec::new())?;
        }
        Ok(flushed)
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
        let done = self.apply("mute_level", None, |r| Ok(r.set_mute_level(level)));
        self.inner.mute_level.store(level, Ordering::SeqCst);
        done.map(|_| ())
    }

    pub fn set_verbosity(&self, v: Verbosity) {
        self.lock().settings.verbosity = v;
    }

    /// When a turn's prose is spoken (#222).
    pub fn set_read_mode(&self, m: ReadMode) {
        self.lock().settings.read_mode = m;
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

    /// Every earcon played from now on (dropping the stream unsubscribes;
    /// senders of streams that went away are pruned at the next earcon or
    /// subscription).
    pub fn subscribe(&self) -> EarconStream {
        let (tx, rx) = channel();
        let alive = Arc::new(());
        let mut subs = self
            .inner
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        subs.retain(|s| s.alive.strong_count() > 0);
        subs.push(Subscriber {
            tx,
            alive: Arc::downgrade(&alive),
        });
        EarconStream { rx, _alive: alive }
    }

    /// Earcon subscribers held now (tests).
    pub fn subscribers(&self) -> usize {
        self.inner
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
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

    /// Report to the trace hook, if any.
    fn trace(&self, source: &str, channel: Option<&str>, what: Traced) {
        let hook = self.trace.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(hook) = hook {
            hook(&Trace {
                source: source.to_string(),
                channel: channel.map(str::to_string),
                what,
            });
        }
    }

    fn lock_seen(&self) -> MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Play an earcon now and report it to the subscribers.
    fn play(&self, e: Earcon) -> Result<()> {
        let clip = self.earcons.clip(e);
        self.channels
            .reader()
            .play_clip(clip.samples.clone(), clip.sample_rate)?;
        self.subscribers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|s| s.alive.strong_count() > 0 && s.tx.send(e).is_ok());
        Ok(())
    }

    fn forget_seen(&self, channel: &str) {
        self.lock_seen().at.remove(channel);
    }

    /// A message for `channel` (or a setting): note it, and now and then
    /// free the turn state of channels silent for `forget_after` (module
    /// docs). Called with the rules locked.
    fn seen(&self, rules: &mut Rules, channel: Option<&str>) {
        let now = Instant::now();
        let mut seen = self.lock_seen();
        if let Some(c) = channel {
            seen.at.insert(c.to_string(), now);
        }
        let every = SWEEP_EVERY.min(self.forget_after);
        if now.duration_since(seen.swept) < every {
            return;
        }
        seen.swept = now;
        let dead: Vec<String> = rules
            .channels()
            .into_iter()
            .filter(|c| {
                seen.at
                    .get(c)
                    .is_none_or(|t| now.duration_since(*t) >= self.forget_after)
            })
            .collect();
        if dead.is_empty() {
            return;
        }
        let focused = self.channels.focused();
        let engaged = self.channels.engaged();
        for c in dead {
            seen.at.remove(&c);
            rules.close(&c);
            let busy = focused.as_deref() == Some(c.as_str())
                || engaged.as_deref() == Some(c.as_str())
                || self.channels.channel(&c).is_none_or(|ch| ch.pending() > 0);
            if !busy {
                let _ = self.channels.close(&c);
            }
        }
    }

    /// Trace the rules' notes, then carry out actions in order; the first
    /// error is returned after the rest were tried (a failed speak must not
    /// lose a later decision).
    fn execute(
        self: &Arc<Self>,
        rules: &Rules,
        source: &str,
        channel: Option<&str>,
        actions: Vec<Action>,
    ) -> Result<()> {
        for n in rules.take_notes() {
            let ch = n.channel.clone();
            self.trace(source, ch.as_deref().or(channel), Traced::Note(n));
        }
        let mut first = Ok(());
        for a in actions {
            if let Err(e) = self.carry_out(rules, source, a) {
                if first.is_ok() {
                    first = Err(e);
                }
            }
        }
        first
    }

    /// Why text added to `channel` may wait (for the trace).
    fn waits(&self, channel: &str, release: bool) -> Option<&'static str> {
        let ch = &self.channels;
        if ch.is_muted(channel) {
            return Some("the session is muted");
        }
        let gated = ch.focus_only() && ch.focused().is_some_and(|f| f != channel) && !release;
        gated.then_some("background policy earcon_only: read once the session is focused")
    }

    fn carry_out(self: &Arc<Self>, rules: &Rules, source: &str, a: Action) -> Result<()> {
        let ch = &self.channels;
        match a {
            Action::Speak {
                channel,
                text,
                decision,
                release,
                kind,
            } => {
                let spoken = ch.add(&channel, &text)?;
                if decision {
                    ch.prioritize(&channel)?;
                }
                if release {
                    ch.authorize(&channel)?;
                }
                let waits = self.waits(&channel, release);
                self.trace(
                    source,
                    Some(&channel),
                    Traced::Spoken {
                        kind,
                        entry: spoken.entry,
                        text,
                        decision,
                        waits,
                    },
                );
            }
            Action::Earcon(e) => {
                self.play(e)?;
                self.trace(source, None, Traced::Earcon(e));
            }
            Action::Wipe { channel, resume } => {
                if ch.channel(&channel).is_none() {
                    return Ok(());
                }
                let reason = if resume { "turn_start" } else { "answered" };
                self.trace(source, Some(&channel), Traced::Wiped { reason });
                let engaged = ch.engaged().as_deref() == Some(channel.as_str());
                ch.control_because(Control::Stop, Some(&channel), reason)?;
                if resume && engaged {
                    let st = ch.reader().state()?;
                    if st.paused && st.now_playing.is_some() {
                        ch.control(Control::Play, None)?;
                    }
                }
            }
            Action::Silence => {
                self.trace(source, None, Traced::Wiped { reason: "mute" });
                ch.control_because(Control::Stop, None, "mute")?
            }
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
        if let Err(e) = self.execute(&rules, "timer", None, actions) {
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
        if let Err(e) = self.execute(&rules, "summary", Some(&job.channel), actions) {
            eprintln!("[agent] summary for {}: {e}", job.channel);
        }
    }
}
