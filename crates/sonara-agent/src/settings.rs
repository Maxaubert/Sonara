//! L3 settings: mute level, verbosity, prose batching and summaries. Ranges
//! and defaults are the Python plugin's (`config_schema.py`).
use std::time::Duration;

/// `set mute_level`: 0 speaks, 1 silences agent speech (earcons still
/// play), 2 also silences the earcons.
pub const MUTE_LEVEL_MAX: u8 = 2;
/// `minqueue` range: prose is held until this many chunks are waiting (or
/// the turn ends); 0 and 1 read at once.
pub const MINQUEUE_MAX: usize = 10;
pub const SUMMARY_TIMEOUT_S: (u64, u64) = (15, 300);
pub const SUMMARY_SETTLE_MS_MAX: u64 = 5_000;

/// How much of the agent's output is spoken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// Prose, decisions, tool announcements and the host's selection hints.
    Everything,
    /// Prose and decisions.
    Medium,
    /// Decisions only (prose is still recorded for summaries).
    Quiet,
}

impl Verbosity {
    pub fn parse(name: &str) -> Option<Verbosity> {
        match name {
            "everything" => Some(Verbosity::Everything),
            "medium" => Some(Verbosity::Medium),
            "quiet" => Some(Verbosity::Quiet),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Verbosity::Everything => "everything",
            Verbosity::Medium => "medium",
            Verbosity::Quiet => "quiet",
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
    pub minqueue: usize,
    pub summaries: SummarySettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mute_level: 0,
            verbosity: Verbosity::Everything,
            minqueue: 1,
            summaries: SummarySettings::default(),
        }
    }
}
