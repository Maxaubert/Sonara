//! An output for tests (feature `test-util`): it plays nothing, records every
//! call, and sends `AudioEvent`s only when the test says so, so a test
//! decides exactly when a chunk starts, finishes or fails.
use crate::{AudioEvent, ItemId, Output, PcmChunk};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

/// One call the output received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputCall {
    Play {
        item: ItemId,
        chunk: usize,
        gen: u64,
        /// Total samples over all PCM chunks.
        samples: usize,
    },
    /// A chunk still being made (#235): more comes with `Append`.
    PlayOpen {
        item: ItemId,
        chunk: usize,
        gen: u64,
        samples: usize,
    },
    Append {
        gen: u64,
        samples: usize,
    },
    Finish {
        gen: u64,
    },
    Pause,
    Resume,
    Stop,
    SetVolume(u8),
    PlayClip {
        samples: usize,
        sample_rate: u32,
    },
}

#[derive(Debug, Default)]
struct Inner {
    calls: Vec<OutputCall>,
    /// gen of the loaded chunk.
    loaded: Option<u64>,
    paused: bool,
    volume: u8,
    /// When set, every play fails at once with this reason (a dead device).
    fail_plays: Option<String>,
}

/// Clones share one recording: give one to the host, keep one in the test.
#[derive(Debug, Clone)]
pub struct TestOutput {
    inner: Arc<Mutex<Inner>>,
    events: Sender<AudioEvent>,
}

impl TestOutput {
    pub fn new() -> (Self, Receiver<AudioEvent>) {
        let (events, rx) = channel();
        let inner = Inner {
            volume: 100,
            ..Inner::default()
        };
        (
            TestOutput {
                inner: Arc::new(Mutex::new(inner)),
                events,
            },
            rx,
        )
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A test that panicked while holding the lock already failed.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every call so far.
    pub fn calls(&self) -> Vec<OutputCall> {
        self.lock().calls.clone()
    }

    /// The calls since the last `take_calls`.
    pub fn take_calls(&self) -> Vec<OutputCall> {
        std::mem::take(&mut self.lock().calls)
    }

    /// gen of the loaded chunk, if any.
    pub fn loaded(&self) -> Option<u64> {
        self.lock().loaded
    }

    pub fn paused(&self) -> bool {
        self.lock().paused
    }

    pub fn volume(&self) -> u8 {
        self.lock().volume
    }

    /// Make every later play fail at once, as a device that is gone.
    pub fn fail_plays(&self, reason: Option<&str>) {
        self.lock().fail_plays = reason.map(str::to_string);
    }

    /// Send any event, also a stale one.
    pub fn send(&self, e: AudioEvent) {
        let _ = self.events.send(e);
    }

    /// The loaded chunk's audio began. Panics if nothing is loaded.
    pub fn start(&self) {
        let gen = self.lock().loaded.expect("start: nothing loaded");
        self.send(AudioEvent::ChunkStarted { gen });
    }

    /// The loaded chunk played to its end and is unloaded.
    pub fn finish(&self) {
        let gen = self.lock().loaded.take().expect("finish: nothing loaded");
        self.send(AudioEvent::ChunkFinished { gen });
    }

    /// The loaded chunk failed and is unloaded.
    pub fn fail(&self, reason: &str) {
        let gen = self.lock().loaded.take().expect("fail: nothing loaded");
        self.send(AudioEvent::Failed {
            gen,
            reason: reason.to_string(),
        });
    }
}

impl Output for TestOutput {
    fn play(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64) {
        let samples = pcm.iter().map(|c| c.samples.len()).sum();
        let mut inner = self.lock();
        inner.calls.push(OutputCall::Play {
            item,
            chunk: chunk_index,
            gen,
            samples,
        });
        inner.paused = false;
        if let Some(reason) = inner.fail_plays.clone() {
            inner.loaded = None;
            drop(inner);
            self.send(AudioEvent::Failed { gen, reason });
        } else {
            inner.loaded = Some(gen);
        }
    }

    fn play_open(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64) {
        let samples = pcm.iter().map(|c| c.samples.len()).sum();
        let mut inner = self.lock();
        inner.calls.push(OutputCall::PlayOpen {
            item,
            chunk: chunk_index,
            gen,
            samples,
        });
        inner.paused = false;
        inner.loaded = Some(gen);
    }

    fn append(&mut self, gen: u64, pcm: Vec<PcmChunk>) {
        let samples = pcm.iter().map(|c| c.samples.len()).sum();
        self.lock().calls.push(OutputCall::Append { gen, samples });
    }

    fn finish(&mut self, gen: u64) {
        self.lock().calls.push(OutputCall::Finish { gen });
    }

    fn pause(&mut self) {
        let mut inner = self.lock();
        inner.calls.push(OutputCall::Pause);
        inner.paused = true;
    }

    fn resume(&mut self) {
        let mut inner = self.lock();
        inner.calls.push(OutputCall::Resume);
        inner.paused = false;
    }

    fn stop(&mut self) {
        let mut inner = self.lock();
        inner.calls.push(OutputCall::Stop);
        inner.loaded = None;
        inner.paused = false;
    }

    fn set_volume(&mut self, percent: u8) {
        let mut inner = self.lock();
        inner.calls.push(OutputCall::SetVolume(percent));
        inner.volume = percent;
    }

    fn play_clip(&mut self, samples: &[i16], sample_rate: u32) {
        self.lock().calls.push(OutputCall::PlayClip {
            samples: samples.len(),
            sample_rate,
        });
    }
}
