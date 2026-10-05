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
//!   finished chunk is spoken in the channel (append) as `read_mode`
//!   says (#222): `immediate` at once; `queue` held below `minqueue` until
//!   the batch is big enough or the turn ends, a tool runs or a decision
//!   arrives (context first); `done` held until the turn ends or a decision
//!   arrives (context first), a tool run does not release it. At verbosity `skip_code` a
//!   code block's summary is recorded but not spoken; summary mode records
//!   prose for the recap.
//! - **turn_end** plays `turn_done` and releases held prose (the turn ends
//!   at this signal, not at a block's `final`).
//! - **Whole messages** (#235, `set_whole_messages`, which the driver sets
//!   from the reader's engine: send mode `message`): every release of prose
//!   is ONE `Speak`, the chunks joined with a space and paragraphs (a blank
//!   line, or a new block) with a blank line, so the engine gets one
//!   request: the turn end in `done`, a batch in `queue`, the prose before
//!   a decision or a tool run (which releases what is held but does not
//!   lift the batch or paragraph rule for the rest of the turn). In
//!   `immediate` the release point is the end
//!   of a paragraph (its blank line or its block's `final`), so reading
//!   starts after the first paragraph and each paragraph is one request.
//!   Decisions and tool announcements stay their own (short) `Speak`: a
//!   decision is read with priority, ahead of other sessions.
//! - **Decisions** (`ask`) are spoken with priority: the driver puts the
//!   channel ahead of the others after the item playing. A question plays
//!   `choice` (once while one is unanswered) and marks the channel as
//!   awaiting an answer; the permission prompt that the same question also
//!   fires is then dropped, earcon and text, and consumes the mark (#11).
//!   A permission otherwise plays `permission`; a plan has no earcon (the
//!   user removed it). A tool running, an answer or a new turn clears the
//!   mark.
//! - **Stop and flush.** `control stop` catches every channel up;
//!   `flush` (the flush hotkey, #228) the session being read: its held
//!   prose, the prose kept for its summary and its summary work are
//!   dropped; the decisions waiting for that summary are spoken now and a
//!   question keeps its mark (a flush answers nothing). Stop drops those
//!   decisions. The rest of the flushed reply is skipped too: prose and
//!   tool announcements arriving before the session's next `turn_start`
//!   are dropped (`dropped: flushed reply`); its decisions are still
//!   spoken. With flush scope `all` the driver also calls `flush_ready`
//!   on every other session whose turn ended (the late prose of a reply it
//!   flushed is skipped the same way); a session still writing its reply
//!   (no `turn_end` yet) is untouched in both scopes.
//! - **Mute levels.** 1 silences agent speech (the driver also silences
//!   what is queued and playing), 2 also drops earcons. Muting never loses
//!   the session's latest message (#243): what would be spoken is
//!   `Action::Store`d instead, kept in the channel as its latest message
//!   but not read, so nothing is synthesized and no engine request goes
//!   out; unmuting reads nothing, and a switch to the session or Up reads
//!   it. Every rule above runs as usual while muted (read modes, whole
//!   messages, turn_start, flush), only the outcome is stored. Summaries
//!   are not made while muted: the prose kept for one is stored as it is
//!   (no summarizer run for text nobody hears now); a summary that lands
//!   while muted is stored.
//! - **Summaries** (opt in): see `Pipeline` below; the rules are the Python
//!   ones: settle window, lead-in digests before decisions, the decision
//!   hold with its cap, the hung-worker watchdog and the reorder buffer.
//! - **Notes** (#219): whatever the rules do not speak now (muted, a code
//!   block at `skip_code`, prose held by `read_mode`, a permission prompt
//!   of an unanswered question, what an answer, a stop or a flush drops,
//!   with the count and the reason, ...) leaves a `Note`, which
//!   the driver takes after each call (`take_notes`) for the
//!   troubleshooting log. Notes never change what is spoken.
use crate::decision::{self, AskKind, Choice};
use crate::earcon::Earcon;
use crate::settings::{ReadMode, Settings, Verbosity};
use sonara_core::assembler::{Chunk, ProseAssembler};
use sonara_core::text::normalize_for_speech;
use std::cell::RefCell;
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
        /// What it is, for the troubleshooting log: `prose`, `question`,
        /// `permission`, `plan`, `tool` or `summary`.
        kind: &'static str,
    },
    /// Play an earcon.
    Earcon(Earcon),
    /// Drop the channel's unread text and cut its item if it is being read.
    /// `resume`: a new turn, which un-pauses the reader when the channel is
    /// the one the user is engaged with (and only then: the pause stays on
    /// when another channel gets a new turn, upstream #69).
    Wipe { channel: String, resume: bool },
    /// Muted (#243): keep `text` in `channel` as its latest message
    /// without reading it (`Channels::store`). Fields as `Speak`.
    Store {
        channel: String,
        text: String,
        decision: bool,
        kind: &'static str,
    },
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

/// Something the rules did not speak now, and why (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub channel: Option<String>,
    /// `prose`, `code`, `question`, `permission`, `plan`, `tool`,
    /// `summary` or `earcon`.
    pub kind: &'static str,
    /// What happened and why (`not spoken: mute level 1`).
    pub what: String,
    /// The text concerned, if any.
    pub text: Option<String>,
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
    kind: &'static str,
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
    /// Prose held by `read_mode`, each chunk with whether it starts a
    /// paragraph (joined with a blank line in whole messages).
    held_prose: Vec<(String, bool)>,
    /// The next chunk starts a paragraph (a blank line or a block ended).
    new_para: bool,
    /// The index of the last delta (its number within its block).
    last_index: Option<u32>,
    /// The turn released its prose (turn_end, or a tool in read mode
    /// `queue`): no more holding.
    released: bool,
    /// The turn ended (`turn_end`): its messages are ready. A channel
    /// whose turn_end has not come is still writing (flush scope `all`
    /// keeps it, #228).
    ended: bool,
    /// The user flushed this reply (#228): the rest of it is skipped until
    /// the next `turn_start`, except its decisions.
    skip_reply: bool,
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
            new_para: false,
            last_index: None,
            released: false,
            ended: false,
            skip_reply: false,
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
    /// Send mode `message` (#235, module docs).
    whole: bool,
    /// Reorder buffer (#88): turn-end summaries are heard in dispatch
    /// order, whatever order the summarizer finishes them in.
    next_seq: u64,
    serve_seq: u64,
    parked: BTreeMap<u64, Release>,
    /// What a hung slot speaks when its watchdog fires.
    watched: HashMap<u64, Release>,
    /// Notes since the last `take_notes` (module docs).
    notes: RefCell<Vec<Note>>,
}

impl Rules {
    pub fn new(settings: Settings) -> Rules {
        Rules {
            settings,
            turns: HashMap::new(),
            next_gen: 0,
            next_decision: 0,
            next_token: 0,
            whole: false,
            next_seq: 0,
            serve_seq: 0,
            parked: BTreeMap::new(),
            watched: HashMap::new(),
            notes: RefCell::new(Vec::new()),
        }
    }

    /// Join every release of prose into one `Speak` (#235, module docs):
    /// the current engine takes whole messages.
    pub fn set_whole_messages(&mut self, on: bool) {
        self.whole = on;
    }

    pub fn whole_messages(&self) -> bool {
        self.whole
    }

    /// The notes left since the last call (module docs).
    pub fn take_notes(&self) -> Vec<Note> {
        std::mem::take(&mut *self.notes.borrow_mut())
    }

    fn note(&self, channel: Option<&str>, kind: &'static str, what: String, text: Option<String>) {
        self.notes.borrow_mut().push(Note {
            channel: channel.map(str::to_string),
            kind,
            what,
            text,
        });
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
        } else {
            self.note(
                None,
                "earcon",
                format!("{} not played: mute level 2", e.as_str()),
                None,
            );
        }
    }

    fn speak(
        &self,
        out: &mut Vec<Action>,
        channel: &str,
        text: String,
        decision: bool,
        kind: &'static str,
    ) {
        self.say(out, channel, text, decision, false, kind);
    }

    fn say(
        &self,
        out: &mut Vec<Action>,
        channel: &str,
        text: String,
        decision: bool,
        release: bool,
        kind: &'static str,
    ) {
        if text.trim().is_empty() {
            return;
        }
        if self.settings.mute_level == 0 {
            out.push(Action::Speak {
                channel: channel.to_string(),
                text,
                decision,
                release,
                kind,
            });
        } else {
            // Kept as the session's latest message, not read (#243).
            out.push(Action::Store {
                channel: channel.to_string(),
                text,
                decision,
                kind,
            });
        }
    }

    /// Drop the prose held by `read_mode`, noted for the troubleshooting
    /// log: in `done` that can be a whole turn.
    fn drop_held(&mut self, channel: &str, why: &str) -> usize {
        let n = std::mem::take(&mut self.turn(channel).held_prose).len();
        if n > 0 {
            let mode = self.settings.read_mode.as_str();
            self.note(
                Some(channel),
                "prose",
                format!("dropped: {n} held chunk(s) ({why}, read_mode {mode})"),
                None,
            );
        }
        n
    }

    /// Speak the prose held by `read_mode`.
    fn flush_prose(&mut self, out: &mut Vec<Action>, channel: &str) {
        let n = self.turn(channel).held_prose.len();
        self.flush_prose_upto(out, channel, n);
    }

    /// Speak the first `n` chunks of the held prose: one `Speak` in whole
    /// messages (module docs), else one per chunk.
    fn flush_prose_upto(&mut self, out: &mut Vec<Action>, channel: &str, n: usize) {
        let c = self.turn(channel);
        let n = n.min(c.held_prose.len());
        let held: Vec<(String, bool)> = c.held_prose.drain(..n).collect();
        if !self.whole {
            for (text, _) in held {
                self.speak(out, channel, text, false, "prose");
            }
            return;
        }
        let mut text = String::new();
        for (t, para) in held {
            if !text.is_empty() {
                text.push_str(if para { "\n\n" } else { " " });
            }
            text.push_str(&t);
        }
        self.speak(out, channel, text, false, "prose");
    }

    // -- messages ---------------------------------------------------------

    /// A delta of streamed prose (`index` numbers the deltas of a block,
    /// restarting at 0 for a new block; `final` ends the block).
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
        let skip_code = self.settings.verbosity == Verbosity::SkipCode;
        let minqueue = self.settings.minqueue;
        let mode = self.settings.read_mode;
        let c = self.turn(channel);
        if c.skip_reply {
            // The user flushed this reply (#228): the rest is skipped.
            let dropped: Vec<(&'static str, String)> = c
                .assembler
                .feed(delta, index, is_final)
                .into_iter()
                .filter_map(|ch| match ch {
                    Chunk::Text(t) => Some(("prose", t)),
                    Chunk::Code(t) => Some(("code", t)),
                    Chunk::ParagraphBreak => None,
                })
                .collect();
            for (kind, t) in dropped {
                self.note(
                    Some(channel),
                    kind,
                    "dropped: flushed reply".into(),
                    Some(t),
                );
            }
            return Ok(out);
        }
        // Every chunk is recorded for summaries; at `skip_code` a code
        // block's announcement is not spoken (#214). Each spoken chunk
        // notes whether it starts a paragraph; `breaks` counts the chunks
        // of this delta before the last paragraph end in it.
        let mut texts: Vec<(String, bool)> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        let mut breaks: Option<usize> = None;
        // `index` numbers the deltas of a message block (0, 1, 2, ...): a
        // new block restarts at 0 and starts a paragraph; the next delta of
        // the same block does not.
        if index == 0 && c.last_index.is_some_and(|i| i != 0) {
            c.new_para = true;
        }
        c.last_index = Some(index);
        for ch in c.assembler.feed(delta, index, is_final) {
            match ch {
                Chunk::Text(t) => {
                    c.prose.push(t.clone());
                    texts.push((t, std::mem::take(&mut c.new_para)));
                }
                Chunk::Code(t) => {
                    c.prose.push(t.clone());
                    if skip_code {
                        skipped.push(t);
                    } else {
                        texts.push((t, std::mem::take(&mut c.new_para)));
                    }
                }
                Chunk::ParagraphBreak => {
                    c.new_para = true;
                    breaks = Some(texts.len());
                }
            }
        }
        if is_final {
            c.new_para = true;
            breaks = Some(texts.len());
        }
        let settle = c.settle_armed;
        for t in skipped {
            self.note(
                Some(channel),
                "code",
                "not spoken: verbosity skip_code".into(),
                Some(t),
            );
        }
        if summaries {
            for (t, _) in texts {
                self.note(
                    Some(channel),
                    "prose",
                    "kept for the summary (summaries on)".into(),
                    Some(t),
                );
            }
            // Late prose after turn_end restarts the settle window, so the
            // summary waits for the whole turn (#14).
            if settle {
                self.arm_settle(&mut out, channel);
            }
            return Ok(out);
        }
        let whole = self.whole;
        // A paragraph that ended in this delta (immediate, whole messages).
        let para_end = whole && mode == ReadMode::Immediate && breaks.is_some();
        if texts.is_empty() && !para_end {
            return Ok(out);
        }
        let c = self.turn(channel);
        let before = c.held_prose.len();
        c.held_prose.extend(texts);
        let ready = c.released
            || match mode {
                ReadMode::Immediate => !whole,
                ReadMode::Queue => c.held_prose.len() >= minqueue,
                ReadMode::Done => false,
            };
        let upto = before + breaks.unwrap_or(0);
        if ready {
            self.flush_prose(&mut out, channel);
        } else if para_end && upto > 0 {
            self.flush_prose_upto(&mut out, channel, upto);
        }
        let held = self.turn(channel).held_prose.len();
        if held > 0 && !ready {
            let what = match mode {
                ReadMode::Done => {
                    format!("held: waits for the turn end (read_mode done), {held} chunk(s)")
                }
                ReadMode::Immediate => format!(
                    "held: {held} chunk(s) wait for the end of the paragraph (send mode message)"
                ),
                ReadMode::Queue => {
                    format!("held: {held} chunk(s) wait for minqueue {minqueue} or the turn end")
                }
            };
            self.note(Some(channel), "prose", what, None);
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
        self.drop_held(channel, "turn_start");
        let c = self.turn(channel);
        c.released = false;
        c.ended = false;
        c.skip_reply = false;
        c.new_para = false;
        c.last_index = None;
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
        let c = self.turn(channel);
        c.released = true;
        c.ended = true;
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
        let (text, kind) = match ask.kind {
            AskKind::Question => {
                if !awaiting {
                    self.earcon(&mut out, Earcon::Choice);
                }
                self.turn(channel).awaiting = true;
                (
                    decision::question_text(&ask.text, &ask.options, ask.multi),
                    "question",
                )
            }
            AskKind::Permission => {
                if awaiting {
                    // The permission prompt the unanswered question also
                    // fires: drop it and consume the mark.
                    self.turn(channel).awaiting = false;
                    self.note(
                        Some(channel),
                        "permission",
                        "not spoken: the permission prompt of the question awaiting its \
                         answer (#11)"
                            .into(),
                        Some(decision::permission_text(&ask.text)),
                    );
                    return out;
                }
                self.earcon(&mut out, Earcon::Permission);
                (decision::permission_text(&ask.text), "permission")
            }
            AskKind::Plan => (decision::plan_text(&ask.text), "plan"),
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
            kind,
        };
        if self.summaries() {
            // The decision can beat its lead-in prose (separate hook
            // processes race): gather the lead-in after the settle window
            // and speak it first (#16).
            self.note(
                Some(channel),
                kind,
                "waits for the settle window (summaries on)".into(),
                Some(item.text.clone()),
            );
            self.turn(channel).pending.push(item);
            self.arm_settle(&mut out, channel);
        } else {
            self.enqueue_or_hold(&mut out, channel, item, false);
        }
        out
    }

    /// A tool runs: the question (if any) was answered; at verbosity
    /// `everything` the tool is announced, after the prose held so far
    /// (read mode `done`: the prose stays held, the tool is announced).
    pub fn tool(&mut self, channel: &str, name: &str, summary: &str) -> Vec<Action> {
        let mut out = Vec::new();
        self.turn(channel).awaiting = false;
        let summary = summary.trim();
        let text = if summary.is_empty() {
            format!("Running {}.", name.trim())
        } else {
            summary.to_string()
        };
        if self.turn(channel).skip_reply {
            self.note(
                Some(channel),
                "tool",
                "not announced: flushed reply".into(),
                Some(text),
            );
            return out;
        }
        if self.settings.verbosity != Verbosity::Everything {
            self.note(
                Some(channel),
                "tool",
                format!(
                    "not announced: verbosity {}",
                    self.settings.verbosity.as_str()
                ),
                Some(text),
            );
            return out;
        }
        if self.settings.read_mode != ReadMode::Done {
            // In whole messages the tool run is one release point; the
            // paragraph and batch rules keep holding after it (#235).
            if !self.whole {
                self.turn(channel).released = true;
            }
            self.flush_prose(&mut out, channel);
        }
        self.speak(&mut out, channel, text, false, "tool");
        out
    }

    /// The user answered the question: what was queued before is stale
    /// (#83). The channel's backlog is skipped and its item cut, summary
    /// work and held decisions are dropped, and a later summary covers only
    /// what comes after the answer. The turn goes on.
    pub fn answered(&mut self, channel: &str) -> Vec<Action> {
        self.turn(channel).awaiting = false;
        let (decisions, _) = self.catch_up(channel, "answered");
        self.drop_decisions(channel, decisions, "answered");
        vec![Action::Wipe {
            channel: channel.to_string(),
            resume: false,
        }]
    }

    /// Skip the channel to now: its held prose, the prose kept for a
    /// summary and its summary work (in flight, parked, settling) are
    /// dropped, each drop noted with `why` (#228). The decisions that
    /// waited for that work are returned for the caller to drop or speak,
    /// with whether anything was there to drop or release. The turn goes
    /// on: what comes later follows the usual rules.
    fn catch_up(&mut self, channel: &str, why: &str) -> (Vec<Decision>, bool) {
        let gen = self.gen();
        let settle = self.gen();
        let held = self.drop_held(channel, why);
        let summaries = self.summaries();
        let c = self.turn(channel);
        let kept = c.prose.len() - c.voiced;
        let (inflight, settling, old) = (c.inflight, c.settle_armed, c.gen);
        let mut decisions: Vec<Decision> = std::mem::take(&mut c.pending);
        decisions.extend(c.held.take().map(|(_, v)| v).unwrap_or_default());
        c.voiced = c.prose.len();
        Self::cancel(c, gen, settle);
        if summaries && kept > 0 {
            self.note(
                Some(channel),
                "prose",
                format!("dropped: {kept} chunk(s) kept for the summary ({why})"),
                None,
            );
        }
        let parked = self
            .parked
            .values()
            .filter(|r| r.channel == channel && r.gen == old)
            .count();
        let mut work = Vec::new();
        if inflight > 0 {
            let s = if inflight == 1 {
                "summary"
            } else {
                "summaries"
            };
            work.push(format!("{inflight} {s} in flight"));
        }
        if parked > 0 {
            let s = if parked == 1 { "summary" } else { "summaries" };
            work.push(format!("{parked} {s} waiting for the earlier ones"));
        }
        if settling {
            work.push("the settle window".to_string());
        }
        let any = held > 0 || (summaries && kept > 0) || !work.is_empty() || !decisions.is_empty();
        if !work.is_empty() {
            self.note(
                Some(channel),
                "summary",
                format!("cancelled ({why}): {}", work.join(", ")),
                None,
            );
        }
        (decisions, any)
    }

    /// Drop `decisions` that waited for summary work, noted with `why`.
    fn drop_decisions(&mut self, channel: &str, decisions: Vec<Decision>, why: &str) {
        for d in decisions {
            self.note(
                Some(channel),
                d.kind,
                format!("dropped: waited for the summary ({why})"),
                Some(d.text),
            );
        }
    }

    /// `control stop` with the extension on: every channel is caught up
    /// (summaries in flight, parked, settling and held decisions are
    /// dropped too, #107). The driver stops L2.
    pub fn stop_all(&mut self) {
        for ch in self.channels() {
            self.turn(&ch).awaiting = false;
            let (decisions, _) = self.catch_up(&ch, "stop");
            self.drop_decisions(&ch, decisions, "stop");
        }
    }

    /// The flush hotkey (#228): `channel`, the session being read, is
    /// caught up and the rest of its reply is skipped: prose and tool
    /// announcements that arrive before its next `turn_start` are dropped
    /// (noted `flushed reply`), while the decisions it asks are still read
    /// (a question needs an answer). Unlike an answer, a flush answers
    /// nothing: a question keeps its awaiting mark (its permission prompt
    /// stays silent, #11), and the decisions that waited for the cancelled
    /// summary are spoken now rather than lost. Other sessions keep their
    /// held prose, summary work and turns still arriving (flush scope
    /// `all` also calls `flush_ready` on them). The driver flushes its L2
    /// channel first. A channel without turn state is left alone.
    pub fn flush(&mut self, channel: &str) -> Vec<Action> {
        if !self.tracks(channel) {
            return Vec::new();
        }
        self.turn(channel).skip_reply = true;
        let (decisions, _) = self.catch_up(channel, "flush");
        self.release(channel, decisions)
    }

    /// Flush scope `all` (#228) for a session other than the one being
    /// read: when its turn ended, its ready work is dropped (held prose,
    /// the prose kept for its summary, its summaries in flight, waiting or
    /// settling) and the decisions that waited for it are spoken now.
    /// `queued` says the driver dropped text of it from its L2 channel.
    /// When anything was flushed, late prose of that reply (#14) is
    /// skipped too, as for the session being read; its next reply is read
    /// as usual. `None` when it is still writing its reply (kept: it is
    /// read when done), has no turn state, or had nothing to drop.
    pub fn flush_ready(&mut self, channel: &str, queued: bool) -> Option<Vec<Action>> {
        if !self.tracks(channel) || self.writing(channel) {
            return None;
        }
        let (decisions, any) = self.catch_up(channel, "flush");
        if !(any || queued) {
            return None;
        }
        self.turn(channel).skip_reply = true;
        Some(self.release(channel, decisions))
    }

    /// Speak the decisions a flush released from the summary they waited
    /// for.
    fn release(&mut self, channel: &str, decisions: Vec<Decision>) -> Vec<Action> {
        let mut out = Vec::new();
        for d in decisions {
            self.note(
                Some(channel),
                d.kind,
                "spoken now: the summary it waited for was flushed".into(),
                Some(d.text.clone()),
            );
            self.speak(&mut out, channel, d.text, true, d.kind);
        }
        out
    }

    /// The channel is still writing its reply: it has turn state and its
    /// `turn_end` has not come since its last `turn_start` (flush scope
    /// `all` keeps it, #228).
    pub fn writing(&self, channel: &str) -> bool {
        self.turns.get(channel).is_some_and(|c| !c.ended)
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
                        self.speak(&mut out, channel, d.text, true, d.kind);
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
        if self.settings.mute_level > 0 {
            // Muted (#243): no summarizer run for text nobody hears now;
            // the prose is stored as it is.
            self.note(
                Some(channel),
                "summary",
                format!(
                    "not made: mute level {}, the prose is stored as it is",
                    self.settings.mute_level
                ),
                None,
            );
            if self.whole {
                self.speak(out, channel, chunks.join(" "), false, "prose");
            } else {
                for chunk in chunks {
                    self.speak(out, channel, chunk, false, "prose");
                }
            }
            return false;
        }
        if text.chars().count() < SUMMARY_MIN_CHARS && !leadin {
            if focused == Some(channel) {
                if self.whole {
                    self.speak(out, channel, chunks.join(" "), false, "prose");
                } else {
                    for chunk in chunks {
                        self.speak(out, channel, chunk, false, "prose");
                    }
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
            let (kind, text) = (item.kind, item.text.clone());
            let mut items = c.held.take().map(|(_, v)| v).unwrap_or_default();
            items.push(item);
            c.held = Some((owner, items));
            self.note(
                Some(channel),
                kind,
                "held behind the summary in flight (context first)".into(),
                Some(text),
            );
            out.push(Action::Timer {
                after: cap,
                timer: Timer::HoldCap {
                    channel: channel.to_string(),
                    decision: id,
                },
            });
        } else {
            self.flush_prose(out, channel);
            self.speak(out, channel, item.text, true, item.kind);
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
            self.speak(&mut out, &job.channel, d.text, true, d.kind);
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
            self.note(
                Some(&r.channel),
                "summary",
                "dropped: the session moved on (a new turn, an answer or a stop)".into(),
                r.summary.or(Some(r.text)),
            );
            return;
        }
        match r.summary.filter(|s| !s.trim().is_empty()) {
            Some(s) => {
                let text = normalize_for_speech(&s);
                self.say(out, &r.channel, text, false, true, "summary");
            }
            None if r.leadin => self.note(
                Some(&r.channel),
                "summary",
                "lead-in dropped: the summarizer gave nothing".into(),
                None,
            ),
            None if r.text.trim().is_empty() => self.earcon(out, Earcon::SummaryFailed),
            None => self.say(out, &r.channel, r.text, false, true, "prose"),
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
