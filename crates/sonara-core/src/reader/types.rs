//! Public types of the reader state machine: inputs, outputs and the state
//! snapshot. Field names follow protocol v1 (spec section 4.1).

/// Identifies one item for its whole life. Ids start at 1 and never repeat
/// within one `Reader`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

/// One text to read, split into its spoken chunks (sentences).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub label: Option<String>,
    /// The text as given to `speak`.
    pub text: String,
    /// Cleaned, speakable chunks; never empty for a queued item.
    pub chunks: Vec<String>,
}

/// How `speak` treats items that are queued but not yet started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMode {
    /// Add after everything already queued.
    Append,
    /// Drop the unread items first (each reported `Skipped`); the current
    /// item keeps playing unless `interrupt` is set.
    Replace,
}

/// Playback controls (spec 4.1 `control`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Play,
    Pause,
    Toggle,
    Stop,
    Skip,
    Previous,
    Next,
    Restart,
    Mute,
    Unmute,
}

/// Lifecycle phase reported by `Event::Item`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemPhase {
    Started,
    Finished,
    Skipped,
    Failed,
}

/// The item being read, as shown in `State`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    pub item_id: ItemId,
    pub label: Option<String>,
    /// The text of the chunk being read (not the whole item).
    pub text: String,
    /// Zero-based index of that chunk.
    pub chunk: usize,
    pub chunks: usize,
}

/// A snapshot of the reader. `seq` is the sequence number of the last
/// `Event::State` emitted (0 before the first one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub seq: u64,
    pub now_playing: Option<NowPlaying>,
    /// Items waiting after the current one.
    pub queued: usize,
    pub paused: bool,
    pub muted: bool,
    /// Speech volume in percent, as set (the host validates the range).
    pub volume: u8,
    /// Speech rate, as set (engine units; the host validates the range).
    pub rate: u32,
    /// Voice id, `None` for the engine default.
    pub voice: Option<String>,
}

/// Something observers of the reader are told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The state changed; emitted only on an actual change, `seq` strictly
    /// increasing.
    State(State),
    Item {
        item_id: ItemId,
        phase: ItemPhase,
    },
}

/// Instructions for the host (engine, audio output, event fan-out), in the
/// order they must be carried out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Produce audio for this chunk and keep it until the item's final
    /// `Event::Item` (Finished, Skipped or Failed). Emitted at most once per
    /// chunk while the item is alive.
    Synthesize {
        item: ItemId,
        chunk: usize,
        text: String,
    },
    /// Play this chunk now (waiting for its synthesis if needed). Report its
    /// `AudioEvent`s tagged with `gen`; events with an older `gen` are
    /// ignored, so a superseded play can never move the reader.
    PlayChunk {
        item: ItemId,
        chunk: usize,
        gen: u64,
    },
    PauseOutput,
    ResumeOutput,
    /// Stop and discard the chunk being played.
    StopOutput,
    /// Set the output volume to zero (state.muted). Playback keeps moving.
    Mute,
    /// Restore the output volume to `state.volume`.
    Unmute,
    /// Apply a new output volume (percent).
    SetVolume(u8),
    Emit(Event),
}

/// What the audio side reports about a `PlayChunk`, tagged with its `gen`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEvent {
    ChunkStarted {
        gen: u64,
    },
    ChunkFinished {
        gen: u64,
    },
    /// Synthesis or playback of the chunk failed.
    Failed {
        gen: u64,
        reason: String,
    },
}
