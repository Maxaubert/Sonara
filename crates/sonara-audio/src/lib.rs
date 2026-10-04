//! Sonara L1 audio output: plays the PCM an engine produced for one chunk,
//! with true pause and resume, volume, and short clips mixed over speech.
//!
//! The reader state machine (`sonara_core::reader`) drives an `Output`
//! through its effects (`PlayChunk`, `PauseOutput`, `ResumeOutput`,
//! `StopOutput`, `Mute`/`Unmute`/`SetVolume`), and the output answers on its
//! event channel with `AudioEvent`s tagged with the `gen` of the play they
//! belong to. Device problems are reported as `AudioEvent::Failed`, never as
//! a panic or an error return: the reader skips the chunk and moves on.
mod rodio_output;

#[cfg(feature = "test-util")]
mod test_output;

pub use rodio_output::RodioOutput;
pub use sonara_core::reader::{AudioEvent, ItemId};
pub use sonara_engine::PcmChunk;

#[cfg(feature = "test-util")]
pub use test_output::{OutputCall, TestOutput};

/// An audio output. Each implementation hands out a
/// `std::sync::mpsc::Receiver<AudioEvent>` when it is created.
///
/// Per `play`, the output sends `ChunkStarted` when the audio begins and
/// `ChunkFinished` when it ended on its own, or `Failed` instead. A `play`
/// replaces whatever was loaded; after `stop` or a replacing `play` the old
/// `gen` sends nothing more that matters (the reader ignores stale gens).
pub trait Output: Send {
    /// Load and start the audio of one chunk (`chunk_index` of `item`).
    fn play(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64);
    /// Load and start the first audio of a chunk that is still being made
    /// (#235, a streaming engine): more comes with `append`, and the chunk
    /// finishes (`ChunkFinished`) only after `finish` and the last audio.
    fn play_open(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64);
    /// More audio of the open chunk `gen` (ignored for any other).
    fn append(&mut self, gen: u64, pcm: Vec<PcmChunk>);
    /// The open chunk `gen` has all its audio: it finishes when played.
    fn finish(&mut self, gen: u64);
    /// Hold the loaded chunk where it is.
    fn pause(&mut self);
    /// Continue the loaded chunk from where it was paused.
    fn resume(&mut self);
    /// Discard the loaded chunk.
    fn stop(&mut self);
    /// Output level in percent (0 is silent; 100 is unity gain). Applies at
    /// once, also to the chunk playing and to clips.
    fn set_volume(&mut self, percent: u8);
    /// Play a short mono clip (an earcon) over the speech, without pausing
    /// or cutting it.
    fn play_clip(&mut self, samples: &[i16], sample_rate: u32);
}
