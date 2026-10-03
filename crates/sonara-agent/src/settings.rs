//! L3 settings: mute level, verbosity, the reading mode and prose batching, the background
//! speech policy and summaries. Ranges and defaults are the Python
//! plugin's (`config_schema.py`).
use std::time::Duration;

/// `set mute_level`: 0 speaks, 1 silences agent speech (earcons still
/// play), 2 also silences the earcons.
pub const MUTE_LEVEL_MAX: u8 = 2;
/// `minqueue` range (read mode `queue`): prose is held until this many
/// chunks are waiting (or the turn ends); 0 and 1 read at once.
pub const MINQUEUE_MAX: usize = 10;
pub const SUMMARY_TIMEOUT_S: (u64, u64) = (15, 300);
pub const SUMMARY_SETTLE_MS_MAX: u64 = 5_000;

/// How much of the agent's output is spoken (#214). Decisions are spoken
/// at both levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// The answer, decisions, tool announcements, the host's selection
    /// hints, and each code block announced ("3-line python code block").
    Everything,
    /// The answer and decisions: code blocks are dropped silently, tools
    /// are not announced and hints are left out.
    SkipCode,
}

impl Verbosity {
    /// A name, or an alias kept for older clients and saved settings:
    /// `all` is `everything`; `medium` and `quiet` (the levels before #214)
    /// are `skip_code`.
    pub fn parse(name: &str) -> Option<Verbosity> {
        match name {
            "everything" | "all" => Some(Verbosity::Everything),
            "skip_code" | "medium" | "quiet" => Some(Verbosity::SkipCode),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Verbosity::Everything => "everything",
            Verbosity::SkipCode => "skip_code",
        }
    }
}

/// When a turn's prose is spoken (`read_mode`, #222). Summaries, when on,
/// own the turn whatever the mode. A decision always speaks the prose
/// held before it first (context first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// Each chunk as it arrives.
    Immediate,
    /// Held until `minqueue` chunks wait; the turn end, a tool run or a
    /// decision releases.
    Queue,
    /// Held until the turn ends or a decision arrives; a tool run does not
    /// release.
    Done,
}

impl ReadMode {
    pub fn parse(name: &str) -> Option<ReadMode> {
        match name {
            "immediate" => Some(ReadMode::Immediate),
            "queue" => Some(ReadMode::Queue),
            "done" => Some(ReadMode::Done),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ReadMode::Immediate => "immediate",
            ReadMode::Queue => "queue",
            ReadMode::Done => "done",
        }
    }
}

/// Which channels may speak (`background_policy`, the Python plugin's
/// `sessions.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundPolicy {
    /// Every channel's prose and decisions are read, one channel at a
    /// time.
    All,
    /// Only the focused channel (the session the user last prompted) is
    /// read; the others play their earcons and their text waits until
    /// the user switches to them, replays them or prompts them (the
    /// Python default). A summary delivery is read whatever the focus.
    EarconOnly,
}

impl BackgroundPolicy {
    pub fn parse(name: &str) -> Option<BackgroundPolicy> {
        match name {
            "all" => Some(BackgroundPolicy::All),
            "earcon_only" => Some(BackgroundPolicy::EarconOnly),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            BackgroundPolicy::All => "all",
            BackgroundPolicy::EarconOnly => "earcon_only",
        }
    }
}

/// The summarizer instruction (`summaries.style`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Everything, rewritten for the ear.
    Tidy,
    /// Cleaned up, noise cut (the default).
    Natural,
    /// The outcome in one to three sentences.
    Brief,
}

impl Style {
    pub fn parse(name: &str) -> Option<Style> {
        match name {
            "tidy" => Some(Style::Tidy),
            "natural" => Some(Style::Natural),
            "brief" => Some(Style::Brief),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Style::Tidy => "tidy",
            Style::Natural => "natural",
            Style::Brief => "brief",
        }
    }
}

/// Which headless agent writes the summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryCommand {
    /// `claude -p` (tools and settings off).
    Claude,
    /// `codex exec` (read-only sandbox).
    Codex,
}

impl SummaryCommand {
    pub fn parse(name: &str) -> Option<SummaryCommand> {
        match name {
            "claude" => Some(SummaryCommand::Claude),
            "codex" => Some(SummaryCommand::Codex),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SummaryCommand::Claude => "claude",
            SummaryCommand::Codex => "codex",
        }
    }
}

/// `set summaries {...}`: a turn's prose is recorded instead of spoken and
/// recapped once the turn settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummarySettings {
    pub enabled: bool,
    pub command: SummaryCommand,
    /// Model alias passed to the command (`haiku`).
    pub model: String,
    /// Seconds before a summarizer call is abandoned (15..=300).
    pub timeout_s: u64,
    /// Quiet time after a turn ends before its summary is asked for.
    pub settle_ms: u64,
    pub style: Style,
    /// A custom instruction replacing the style's built-in one.
    pub prompt: Option<String>,
}

impl Default for SummarySettings {
    fn default() -> Self {
        SummarySettings {
            enabled: false,
            command: SummaryCommand::Claude,
            model: "haiku".into(),
            timeout_s: 60,
            settle_ms: 600,
            style: Style::Natural,
            prompt: None,
        }
    }
}

impl SummarySettings {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_s)
    }

    /// How long a decision waits for its lead-in summary before it is
    /// spoken anyway: the summarizer timeout plus a grace for the worker to
    /// finish (the wedge guard of #83/#121, derived so a timeout change
    /// carries it along).
    pub fn hold_cap(&self) -> Duration {
        self.timeout() + Duration::from_secs(5)
    }

    /// When a turn-end summary still out is spoken raw (twice the timeout:
    /// the summarizer enforces its timeout, so a worker still out then is
    /// hung, #138).
    pub fn watchdog(&self) -> Duration {
        self.timeout() * 2
    }

    pub fn settle(&self) -> Duration {
        Duration::from_millis(self.settle_ms)
    }
}

/// Every L3 setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub mute_level: u8,
    pub verbosity: Verbosity,
    pub read_mode: ReadMode,
    pub minqueue: usize,
    pub background: BackgroundPolicy,
    pub summaries: SummarySettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mute_level: 0,
            verbosity: Verbosity::Everything,
            read_mode: ReadMode::Queue,
            minqueue: 1,
            background: BackgroundPolicy::EarconOnly,
            summaries: SummarySettings::default(),
        }
    }
}
