//! The pure L3 rules: turns, streamed prose, decisions, earcons, mute and
//! summaries. No threads, no clock, no reader: every input returns the
//! `Action`s the driver (`Agent`) carries out with L2, and time comes in as
//! the `Timer`s it asked for. Ported from the Python daemon's `ingest.py`,
//! `controls.py` and `summary/` (pipeline and reorder buffer); issue numbers
//! name the Python regressions each rule guards.
//!
//! Rules (binding for `Agent` and the `agent` protocol extension):
//! - **Turns.** `turn_start` begins a new turn on a channel: its unread
//!   text is dropped and its item cut (`Action::Wipe`), the prose assembler
//!   restarts, summary work for the old turn is cancelled and a question
//!   awaiting an answer is forgotten. A message stamped with `t` (its
//!   sender's start time) older than the channel's last `turn_start` is
//!   late text from the previous turn and is dropped (#174), as is one
//!   naming an earlier `turn`; unstamped messages are never stale. A
//!   `turn_start` older than the last one is itself dropped.
//! - **Prose** (`stream`) goes through the streaming assembler; each
//!   finished chunk is spoken in the channel (append), held below
//!   `minqueue` until the batch is big enough or the turn ends, a tool runs
//!   or a decision arrives (context first). Verbosity `quiet` records it
//!   without speaking; summary mode records it for the recap.
//! - **turn_end** plays `turn_done` and releases held prose (the turn ends
//!   at this signal, not at a block's `final`).
//! - **Decisions** (`ask`) are spoken with priority: the driver puts the
//!   channel ahead of the others after the item playing. A question plays
//!   `choice` (once while one is unanswered) and marks the channel as
//!   awaiting an answer; the permission prompt that the same question also
//!   fires is then dropped, earcon and text, and consumes the mark (#11).
//!   A permission otherwise plays `permission`; a plan has no earcon (the
//!   user removed it). A tool running, an answer or a new turn clears the
//!   mark.
//! - **Mute levels.** 1 drops agent speech (the driver also silences what
//!   is queued and playing), 2 also drops earcons.
//! - **Summaries** (opt in): see `Pipeline` below; the rules are the Python
//!   ones: settle window, lead-in digests before decisions, the decision
//!   hold with its cap, the hung-worker watchdog and the reorder buffer.
use crate::decision::{self, AskKind, Choice};
use crate::earcon::Earcon;
use crate::settings::{Settings, Verbosity};
use sonara_core::assembler::{Chunk, ProseAssembler};
use sonara_core::text::normalize_for_speech;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Duration;

/// A turn whose unvoiced prose is shorter than this (in characters) is
/// spoken as it is rather than summarized (a recap of a short message adds
/// nothing and risks spoken meta-text). A lead-in before a decision is
/// summarized even when short (#83).
pub const SUMMARY_MIN_CHARS: usize = 280;

/// Turn ids remembered per channel to drop their late text.
const RETIRED_TURNS: usize = 16;

/// What the driver carries out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Speak `text` in `channel` (appended to its batch). A decision is read
    /// before the other channels. A `release` (a summary, or the raw text
    /// standing in for one) is read whatever the background policy (the
    /// Python digest delivery's `authorize_replay`).
    Speak {
        channel: String,
        text: String,
        decision: bool,
        release: bool,
    },
    /// Play an earcon.
    Earcon(Earcon),
    /// Drop the channel's unread text and cut its item if it is being read.
    /// `resume`: a new turn, which un-pauses the reader when the channel is
    /// the one the user is engaged with (and only then: the pause stays on
    /// when another channel gets a new turn, upstream #69).
    Wipe { channel: String, resume: bool },
    /// Mute: drop everything unread and cut the item playing.
    Silence,
    /// Run the summarizer on a job; answer with `Rules::digest_done`.
    Summarize(Job),
    /// Call `Rules::fire(timer)` after `after`.
    Timer { after: Duration, timer: Timer },
}

/// A timer the rules asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Timer {
    /// The turn's settle window elapsed.
    Settle { channel: String, gen: u64 },
    /// The hold cap of a decision waiting for its lead-in summary.
    HoldCap { channel: String, decision: u64 },
    /// A turn-end summary is still out at twice the timeout.
    Watchdog { seq: u64 },
}

/// One summarizer call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub token: u64,
    pub channel: String,
    /// The channel's cancel generation at dispatch.
    pub gen: u64,
    pub text: String,
    /// A lead-in before decisions (dropped when it comes back empty).
    pub leadin: bool,
    /// Its slot in the reorder buffer (turn-end summaries only).
    pub seq: Option<u64>,
}

/// The message came from an earlier turn and was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stale;

/// An `ask`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub kind: AskKind,
    pub text: String,
    pub options: Vec<Choice>,
    /// A question that takes several options.
    pub multi: bool,
    /// Always spoken after the decision (how to answer this one).
    pub notes: Option<String>,
    /// Spoken after the notes at verbosity `everything`.
    pub hint: Option<String>,
    /// Spoken after the hint the first time a channel gets one.
    pub hint_once: Option<String>,
}

impl Ask {
    pub fn new(kind: AskKind, text: &str) -> Ask {
        Ask {
            kind,
            text: text.to_string(),
            options: Vec::new(),
            multi: false,
            notes: None,
            hint: None,
            hint_once: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Decision {
    id: u64,
    text: String,
}

/// A summary (or raw text) waiting for its turn to be spoken.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Release {
    channel: String,
    gen: u64,
    text: String,
    leadin: bool,
    summary: Option<String>,
}

/// One channel's turn state.
struct Turn {
    assembler: ProseAssembler,
    /// Start time of the last turn_start.
    t: Option<f64>,
    current: Option<String>,
    retired: VecDeque<String>,
    /// A question is unanswered (suppresses its permission prompt).
    awaiting: bool,
    /// `hint_once` was spoken in this channel.
    hinted: bool,
    /// Prose held below `minqueue`.
    held_prose: Vec<String>,
    /// The turn released its prose (turn_end or a tool): no more holding.
    released: bool,
    /// This turn's prose chunks, and how many were voiced by a summary.
    prose: Vec<String>,
    voiced: usize,
    /// Cancel generation: moved by a new turn, an answer or a stop, so a
    /// summary dispatched before is dropped when it lands (#13). Unique
    /// over all channels, so a closed and reopened channel never matches.
    gen: u64,
    settle_gen: u64,
    settle_armed: bool,
    /// Decisions waiting for the settle window (summary mode, #16).
    pending: Vec<Decision>,
    /// Decisions held behind a summary in flight: (owner token, items).
    held: Option<(u64, Vec<Decision>)>,
    inflight: u32,
    last_token: u64,
}

impl Turn {
    fn new(gen: u64) -> Turn {
        Turn {
            assembler: ProseAssembler::new(),
            t: None,
            current: None,
            retired: VecDeque::new(),
            awaiting: false,
            hinted: false,
            held_prose: Vec::new(),
            released: false,
            prose: Vec::new(),
            voiced: 0,
            gen,
            settle_gen: 0,
            settle_armed: false,
            pending: Vec::new(),
            held: None,
            inflight: 0,
            last_token: 0,
        }
    }

    fn stale(&self, turn: Option<&str>, t: Option<f64>) -> bool {
        if let (Some(t), Some(start)) = (t, self.t) {
            if t < start {
                return true;
            }
        }
        turn.is_some_and(|id| self.retired.iter().any(|r| r == id))
    }
}

/// See the module docs.
pub struct Rules {
    pub settings: Settings,
    turns: HashMap<String, Turn>,
    next_gen: u64,
    next_decision: u64,
    next_token: u64,
    /// Reorder buffer (#88): turn-end summaries are heard in dispatch
    /// order, whatever order the summarizer finishes them in.
    next_seq: u64,
    serve_seq: u64,
    parked: BTreeMap<u64, Release>,
    /// What a hung slot speaks when its watchdog fires.
    watched: HashMap<u64, Release>,
}

impl Rules {
    pub fn new(settings: Settings) -> Rules {
        Rules {
            settings,
            turns: HashMap::new(),
            next_gen: 0,
            next_decision: 0,
            next_token: 0,
            next_seq: 0,
            serve_seq: 0,
            parked: BTreeMap::new(),
            watched: HashMap::new(),
        }
    }

    fn gen(&mut self) -> u64 {
        self.next_gen += 1;
        self.next_gen
    }

    fn turn(&mut self, channel: &str) -> &mut Turn {
        if !self.turns.contains_key(channel) {
            let gen = self.gen();
            self.turns.insert(channel.to_string(), Turn::new(gen));
        }
        self.turns.get_mut(channel).expect("inserted")
    }

    /// Channels with L3 state.
    pub fn channels(&self) -> Vec<String> {
        let mut v: Vec<String> = self.turns.keys().cloned().collect();
        v.sort();
        v
    }

    /// The channel awaits an answer to a question.
    pub fn awaiting(&self, channel: &str) -> bool {
        self.turns.get(channel).is_some_and(|t| t.awaiting)
    }

    fn summaries(&self) -> bool {
        self.settings.summaries.enabled
    }

    fn earcon(&self, out: &mut Vec<Action>, e: Earcon) {
        if self.settings.mute_level < 2 {
            out.push(Action::Earcon(e));
        }
    }

    fn speak(&self, out: &mut Vec<Action>, channel: &str, text: String, decision: bool) {
        self.say(out, channel, text, decision, false);
    }

    fn say(
        &self,
        out: &mut Vec<Action>,
        channel: &str,
        text: String,
        decision: bool,
        release: bool,
    ) {
        if self.settings.mute_level == 0 && !text.trim().is_empty() {
            out.push(Action::Speak {
                channel: channel.to_string(),
                text,
                decision,
                release,
            });
        }
    }

    /// Speak the prose held below `minqueue`.
    fn flush_prose(&mut self, out: &mut Vec<Action>, channel: &str) {
        let held = std::mem::take(&mut self.turn(channel).held_prose);
        for text in held {
            self.speak(out, channel, text, false);
        }
    }

    // -- messages ---------------------------------------------------------

    /// A delta of streamed prose (`index` is the block, `final` ends it).
    pub fn stream(
        &mut self,
        channel: &str,
        turn: Option<&str>,
        delta: &str,
        index: u32,
        is_final: bool,
        t: Option<f64>,
    ) -> Result<Vec<Action>, Stale> {
        if self.turns.get(channel).is_some_and(|c| c.stale(turn, t)) {
            return Err(Stale);
        }
        let mut out = Vec::new();
        let summaries = self.summaries();
        let quiet = self.settings.verbosity == Verbosity::Quiet;
        let minqueue = self.settings.minqueue;
        let c = self.turn(channel);
        let texts: Vec<String> = c
            .assembler
            .feed(delta, index, is_final)
            .into_iter()
            .filter_map(|ch| match ch {
                Chunk::Text(t) => Some(t),
                Chunk::ParagraphBreak => None,
            })
            .collect();
        c.prose.extend(texts.iter().cloned());
        if summaries {
            // Late prose after turn_end restarts the settle window, so the
            // summary waits for the whole turn (#14).
            if c.settle_armed {
                self.arm_settle(&mut out, channel);
            }
            return Ok(out);
        }
        if quiet || texts.is_empty() {
            return Ok(out);
        }
        c.held_prose.extend(texts);
        if c.released || c.held_prose.len() >= minqueue {
            self.flush_prose(&mut out, channel);
        }
        Ok(out)
    }

    /// A new turn (the user's prompt).
    pub fn turn_start(
        &mut self,
        channel: &str,
        turn: Option<&str>,
        t: Option<f64>,
    ) -> Result<Vec<Action>, Stale> {
        if self.turns.get(channel).is_some_and(|c| c.stale(turn, t)) {
            return Err(Stale);
        }
        let gen = self.gen();
        let settle = self.gen();
        let c = self.turn(channel);
        c.assembler = ProseAssembler::new();
        if t.is_some() {
            c.t = t;
        }
        if let Some(old) = c.current.take() {
            if turn != Some(old.as_str()) {
                c.retired.push_back(old);
                if c.retired.len() > RETIRED_TURNS {
                    c.retired.pop_front();
                }
            }
        }
        c.current = turn.map(str::to_string);
        c.awaiting = false;
        c.held_prose.clear();
        c.released = false;
        c.prose.clear();
        c.voiced = 0;
        Self::cancel(c, gen, settle);
        Ok(vec![Action::Wipe {
            channel: channel.to_string(),
            resume: true,
        }])
    }

    /// Cancel the channel's summary work: in-flight and parked summaries
    /// land dead, the settle window is off, deferred and held decisions are
    /// dropped.
    fn cancel(c: &mut Turn, gen: u64, settle_gen: u64) {
        c.gen = gen;
        c.settle_gen = settle_gen;
        c.settle_armed = false;
        c.pending.clear();
        c.held = None;
        c.inflight = 0;
        c.last_token = 0;
    }

    /// The agent finished its turn.
    pub fn turn_end(
        &mut self,
        channel: &str,
        turn: Option<&str>,
        t: Option<f64>,
    ) -> Result<Vec<Action>, Stale> {
        if self.turns.get(channel).is_some_and(|c| c.stale(turn, t)) {
            return Err(Stale);
        }
        let mut out = Vec::new();
        self.earcon(&mut out, Earcon::TurnDone);
        self.turn(channel).released = true;
        self.flush_prose(&mut out, channel);
        if self.summaries() {
            // Not yet: the turn's last prose can arrive after this (#14).
            self.arm_settle(&mut out, channel);
        }
        Ok(out)
    }

    /// A decision blocks the agent.
    pub fn ask(&mut self, channel: &str, ask: &Ask) -> Vec<Action> {
        let mut out = Vec::new();
        let everything = self.settings.verbosity == Verbosity::Everything;
        let awaiting = self.turn(channel).awaiting;
        let text = match ask.kind {
            AskKind::Question => {
                if !awaiting {
                    self.earcon(&mut out, Earcon::Choice);
                }
                self.turn(channel).awaiting = true;
                decision::question_text(&ask.text, &ask.options, ask.multi)
            }
            AskKind::Permission => {
                if awaiting {
                    // The permission prompt the unanswered question also
                    // fires: drop it and consume the mark.
                    self.turn(channel).awaiting = false;
                    return out;
                }
                self.earcon(&mut out, Earcon::Permission);
                decision::permission_text(&ask.text)
            }
            AskKind::Plan => decision::plan_text(&ask.text),
        };
        let mut extras: Vec<&str> = Vec::new();
        if let Some(n) = &ask.notes {
            extras.push(n);
        }
        if everything {
            if let Some(h) = &ask.hint {
                extras.push(h);
            }
            if let Some(once) = &ask.hint_once {
                let c = self.turn(channel);
                if !c.hinted {
                    c.hinted = true;
                    extras.push(once);
                }
            }
        }
        let text = decision::with_extras(text, &extras);
        self.next_decision += 1;
        let item = Decision {
            id: self.next_decision,
            text,
        };
        if self.summaries() {
            // The decision can beat its lead-in prose (separate hook
            // processes race): gather the lead-in after the settle window
            // and speak it first (#16).
            self.turn(channel).pending.push(item);
            self.arm_settle(&mut out, channel);
        } else {
            self.enqueue_or_hold(&mut out, channel, item, false);
        }
        out
    }

    /// A tool runs: the question (if any) was answered; at verbosity
    /// `everything` the tool is announced, after the prose held so far.
    pub fn tool(&mut self, channel: &str, name: &str, summary: &str) -> Vec<Action> {
        let mut out = Vec::new();
        self.turn(channel).awaiting = false;
        if self.settings.verbosity != Verbosity::Everything {
            return out;
        }
        self.turn(channel).released = true;
        self.flush_prose(&mut out, channel);
        let summary = summary.trim();
        let text = if summary.is_empty() {
            format!("Running {}.", name.trim())
        } else {
            summary.to_string()
        };
        self.speak(&mut out, channel, text, false);
        out
    }

    /// The user answered the question: what was queued before is stale
    /// (#83). The channel's backlog is skipped and its item cut, summary
    /// work and held decisions are dropped, and a later summary covers only
    /// what comes after the answer. The turn goes on.
    pub fn answered(&mut self, channel: &str) -> Vec<Action> {
        self.catch_up(channel);
        vec![Action::Wipe {
            channel: channel.to_string(),
            resume: false,
        }]
    }

    fn catch_up(&mut self, channel: &str) {
        let gen = self.gen();
        let settle = self.gen();
        let c = self.turn(channel);
        c.awaiting = false;
        c.held_prose.clear();
        c.voiced = c.prose.len();
        Self::cancel(c, gen, settle);
    }

    /// `control stop` with the extension on: every channel is caught up
    /// (summaries in flight, parked, settling and held decisions are
    /// dropped too, #107). The driver stops L2.
    pub fn stop_all(&mut self) {
        for ch in self.channels() {
            self.catch_up(&ch);
        }
    }

    /// The channel closed (or was forgotten): free its turn state. Summary
    /// work still out lands dead.
    pub fn close(&mut self, channel: &str) {
        self.turns.remove(channel);
        // Turn-end slots of the channel still waiting land dead too, so a
        // watchdog that fires later finds nothing to speak.
        self.watched.retain(|_, r| r.channel != channel);
    }

    /// The channel has turn state.
    pub fn tracks(&self, channel: &str) -> bool {
        self.turns.contains_key(channel)
    }

    /// `earcon {kind}`.
    pub fn play(&self, e: Earcon) -> Vec<Action> {
        let mut out = Vec::new();
        self.earcon(&mut out, e);
        out
    }

    /// `set mute_level`. Muting silences what is queued and playing.
    pub fn set_mute_level(&mut self, level: u8) -> Vec<Action> {
        self.settings.mute_level = level.min(crate::settings::MUTE_LEVEL_MAX);
        if self.settings.mute_level >= 1 {
            vec![Action::Silence]
        } else {
            Vec::new()
        }
    }

    // -- summaries ----------------------------------------------------------

    fn arm_settle(&mut self, out: &mut Vec<Action>, channel: &str) {
        let gen = self.gen();
        let after = self.settings.summaries.settle();
        let c = self.turn(channel);
        c.settle_gen = gen;
        c.settle_armed = true;
        out.push(Action::Timer {
            after,
            timer: Timer::Settle {
                channel: channel.to_string(),
                gen,
            },
        });
    }

    /// A timer fired. `focused` is the channel the user works in (its
    /// short turns are spoken at once rather than through the reorder
    /// buffer).
    pub fn fire(&mut self, timer: &Timer, focused: Option<&str>) -> Vec<Action> {
        let mut out = Vec::new();
        match timer {
            Timer::Settle { channel, gen } => {
                let Some(c) = self.turns.get_mut(channel) else {
                    return out;
                };
                if c.settle_gen != *gen || !c.settle_armed {
                    return out;
                }
                c.settle_armed = false;
                let items = std::mem::take(&mut c.pending);
                if items.is_empty() {
                    self.maybe_summarize(&mut out, channel, false, focused);
                } else {
                    let digesting = self.maybe_summarize(&mut out, channel, true, focused);
                    for item in items {
                        self.enqueue_or_hold(&mut out, channel, item, digesting);
                    }
                }
            }
            Timer::HoldCap { channel, decision } => {
                let Some(c) = self.turns.get_mut(channel) else {
                    return out;
                };
                let holds = c
                    .held
                    .as_ref()
                    .is_some_and(|(_, items)| items.iter().any(|d| d.id == *decision));
                if holds {
                    // The summary is still out: the decision speaks now and
                    // the summary follows (bounded inversion).
                    let (_, items) = c.held.take().expect("checked");
                    for d in items {
                        self.speak(&mut out, channel, d.text, true);
                    }
                }
            }
            Timer::Watchdog { seq } => {
                if let Some(release) = self.watched.remove(seq) {
                    if *seq >= self.serve_seq && !self.parked.contains_key(seq) {
                        self.land(&mut out, Some(*seq), release);
                    }
                }
            }
        }
        out
    }

    /// Recap the channel's prose not yet voiced this turn. Short turn-end
    /// prose is spoken as it is; otherwise a summarizer job goes out.
    /// Returns true when a job went out (decisions then wait for it).
    fn maybe_summarize(
        &mut self,
        out: &mut Vec<Action>,
        channel: &str,
        leadin: bool,
        focused: Option<&str>,
    ) -> bool {
        if !self.summaries() {
            return false;
        }
        let c = self.turn(channel);
        let chunks: Vec<String> = c.prose[c.voiced..].to_vec();
        let text = chunks.join(" ").trim().to_string();
        if text.is_empty() {
            return false;
        }
        c.voiced = c.prose.len();
        let gen = c.gen;
        if text.chars().count() < SUMMARY_MIN_CHARS && !leadin {
            if focused == Some(channel) {
                for chunk in chunks {
                    self.speak(out, channel, chunk, false);
                }
            } else {
                // Joins the summary sequence, so a short turn finishing
                // after a long one does not overtake its summary (#88).
                let seq = self.alloc();
                let release = Release {
                    channel: channel.to_string(),
                    gen,
                    text,
                    leadin: false,
                    summary: None,
                };
                self.land(out, Some(seq), release);
            }
            return false;
        }
        self.next_token += 1;
        let token = self.next_token;
        let c = self.turn(channel);
        c.last_token = token;
        c.inflight += 1;
        let seq = if leadin { None } else { Some(self.alloc()) };
        if let Some(seq) = seq {
            self.watched.insert(
                seq,
                Release {
                    channel: channel.to_string(),
                    gen,
                    text: text.clone(),
                    leadin: false,
                    summary: None,
                },
            );
            out.push(Action::Timer {
                after: self.settings.summaries.watchdog(),
                timer: Timer::Watchdog { seq },
            });
        }
        out.push(Action::Summarize(Job {
            token,
            channel: channel.to_string(),
            gen,
            text,
            leadin,
            seq,
        }));
        true
    }

    /// Speak a decision now, or hold it behind the summary in flight
    /// (context first, #16/#21); the hold is capped.
    fn enqueue_or_hold(
        &mut self,
        out: &mut Vec<Action>,
        channel: &str,
        item: Decision,
        digesting: bool,
    ) {
        let cap = self.settings.summaries.hold_cap();
        let c = self.turn(channel);
        if digesting || c.inflight > 0 {
            let owner = c.last_token;
            let id = item.id;
            let mut items = c.held.take().map(|(_, v)| v).unwrap_or_default();
            items.push(item);
            c.held = Some((owner, items));
            out.push(Action::Timer {
                after: cap,
                timer: Timer::HoldCap {
                    channel: channel.to_string(),
                    decision: id,
                },
            });
        } else {
            self.flush_prose(out, channel);
            self.speak(out, channel, item.text, true);
        }
    }

    /// The summarizer answered a job (`None`: failed, empty or SKIP).
    pub fn digest_done(&mut self, job: &Job, summary: Option<String>) -> Vec<Action> {
        let mut out = Vec::new();
        let mut held = Vec::new();
        if let Some(c) = self.turns.get_mut(&job.channel) {
            if c.held
                .as_ref()
                .is_some_and(|(owner, _)| *owner == job.token)
            {
                held = c.held.take().map(|(_, v)| v).unwrap_or_default();
            }
            if c.gen == job.gen {
                c.inflight = c.inflight.saturating_sub(1);
            }
        }
        let release = Release {
            channel: job.channel.clone(),
            gen: job.gen,
            text: job.text.clone(),
            leadin: job.leadin,
            summary,
        };
        self.land(&mut out, job.seq, release);
        for d in held {
            // Decisions after their context.
            self.speak(&mut out, &job.channel, d.text, true);
        }
        out
    }

    fn alloc(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// Park a release under its slot and release every consecutive ready
    /// slot. `None` bypasses the order (lead-ins). A slot already served (a
    /// worker answering after its watchdog) is ignored.
    fn land(&mut self, out: &mut Vec<Action>, seq: Option<u64>, release: Release) {
        let Some(seq) = seq else {
            self.apply(out, release);
            return;
        };
        if seq < self.serve_seq || self.parked.contains_key(&seq) {
            return;
        }
        self.watched.remove(&seq);
        self.parked.insert(seq, release);
        while let Some(r) = self.parked.remove(&self.serve_seq) {
            self.serve_seq += 1;
            self.apply(out, r);
        }
    }

    /// Speak a released summary: dropped if its channel moved on since
    /// dispatch; an empty summary falls back to the raw text (a turn's last
    /// message is always read), except for a lead-in, which is dropped.
    fn apply(&mut self, out: &mut Vec<Action>, r: Release) {
        if self.turns.get(&r.channel).map(|c| c.gen) != Some(r.gen) {
            return;
        }
        match r.summary.filter(|s| !s.trim().is_empty()) {
            Some(s) => {
                let text = normalize_for_speech(&s);
                self.say(out, &r.channel, text, false, true);
            }
            None if r.leadin => {}
            None if r.text.trim().is_empty() => self.earcon(out, Earcon::SummaryFailed),
            None => self.say(out, &r.channel, r.text, false, true),
        }
    }

    /// Summary work in flight or waiting, for tests and diagnostics:
    /// (settling, pending decisions, held decisions, in flight).
    pub fn summary_state(&self, channel: &str) -> Option<(bool, usize, usize, u32)> {
        self.turns.get(channel).map(|c| {
            (
                c.settle_armed,
                c.pending.len(),
                c.held.as_ref().map_or(0, |(_, v)| v.len()),
                c.inflight,
            )
        })
    }
}
