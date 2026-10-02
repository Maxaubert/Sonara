//! The effect loop: carries out the reader's effects with an engine and an
//! output, and feeds the output's events back to the reader. Single
//! threaded and synchronous (synthesis blocks the loop), which is enough
//! for the `say` example and makes tests deterministic. Shared by
//! `examples/say.rs` and `tests/effect_loop.rs`; the threaded facade that
//! hosts use is `sonara-reader` (M5).
#![allow(dead_code)]
use sonara_audio::{AudioEvent, Output, PcmChunk};
use sonara_core::reader::{Control, Effect, Event, ItemId, ItemPhase, QueueMode, Reader};
use sonara_engine::Engine;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

/// One thing that happened in the loop, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// An effect the reader returned, carried out.
    Fx(Effect),
    /// An event fed back to the reader.
    Audio(AudioEvent),
}

pub struct Driver<O: Output> {
    pub reader: Reader,
    engine: Arc<dyn Engine>,
    pub output: O,
    events: Receiver<AudioEvent>,
    /// Synthesized audio (or the failure) per chunk, until its item ends.
    audio: HashMap<(ItemId, usize), Result<Vec<PcmChunk>, String>>,
    /// Failures found while carrying out effects, reported to the reader
    /// after the current batch (a failed synthesis surfaces at its play).
    pending: VecDeque<AudioEvent>,
    volume: u8,
    muted: bool,
    /// Everything that happened, for tests and logging.
    pub log: Vec<Step>,
    /// Synthesis errors, in order, with the chunk text.
    pub failures: Vec<(String, String)>,
}

impl<O: Output> Driver<O> {
    pub fn new(engine: Arc<dyn Engine>, output: O, events: Receiver<AudioEvent>) -> Self {
        let reader = Reader::new();
        let volume = reader.state().volume;
        Driver {
            reader,
            engine,
            output,
            events,
            audio: HashMap::new(),
            pending: VecDeque::new(),
            volume,
            muted: false,
            log: Vec::new(),
            failures: Vec::new(),
        }
    }

    pub fn speak(&mut self, text: &str) -> ItemId {
        let (id, fx) = self.reader.speak(text, QueueMode::Append, false, None);
        self.run(fx);
        id
    }

    pub fn control(&mut self, c: Control) {
        let fx = self.reader.control(c);
        self.run(fx);
    }

    /// Call a reader method that returns effects (`set_volume`, `set_rate`...).
    pub fn with_reader(&mut self, f: impl FnOnce(&mut Reader) -> Vec<Effect>) {
        let fx = f(&mut self.reader);
        self.run(fx);
    }

    /// Handle every event the output has already sent. Returns how many.
    pub fn pump(&mut self) -> usize {
        let mut n = 0;
        while let Ok(e) = self.events.try_recv() {
            self.audio_event(e);
            n += 1;
        }
        n
    }

    /// Wait up to `timeout` for an event, then handle it and any others
    /// already queued. False on timeout or when the output is gone.
    pub fn wait(&mut self, timeout: Duration) -> bool {
        match self.events.recv_timeout(timeout) {
            Ok(e) => {
                self.audio_event(e);
                self.pump();
                true
            }
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => false,
        }
    }

    /// Nothing playing and nothing queued.
    pub fn idle(&self) -> bool {
        let s = self.reader.state();
        s.now_playing.is_none() && s.queued == 0
    }

    /// Chunks whose audio is kept (for items still alive).
    pub fn cached(&self) -> usize {
        self.audio.len()
    }

    fn audio_event(&mut self, e: AudioEvent) {
        self.log.push(Step::Audio(e.clone()));
        let fx = self.reader.on_audio(e);
        self.run(fx);
    }

    fn run(&mut self, fx: Vec<Effect>) {
        for f in fx {
            self.log.push(Step::Fx(f.clone()));
            self.apply(f);
        }
        while let Some(e) = self.pending.pop_front() {
            self.audio_event(e);
        }
    }

    fn apply(&mut self, f: Effect) {
        match f {
            Effect::Synthesize { item, chunk, text } => {
                let state = self.reader.state();
                let voice = state.voice.unwrap_or_default();
                let result = self
                    .engine
                    .synthesize(&text, &voice, state.rate)
                    .and_then(|stream| stream.collect::<Result<Vec<_>, _>>())
                    .map_err(|e| e.to_string());
                if let Err(e) = &result {
                    self.failures.push((text, e.clone()));
                }
                self.audio.insert((item, chunk), result);
            }
            Effect::PlayChunk { item, chunk, gen } => match self.audio.get(&(item, chunk)) {
                Some(Ok(pcm)) => self.output.play(pcm.clone(), item, chunk, gen),
                Some(Err(reason)) => self.pending.push_back(AudioEvent::Failed {
                    gen,
                    reason: reason.clone(),
                }),
                None => self.pending.push_back(AudioEvent::Failed {
                    gen,
                    reason: "chunk was never synthesized".into(),
                }),
            },
            Effect::PauseOutput => self.output.pause(),
            Effect::ResumeOutput => self.output.resume(),
            Effect::StopOutput => self.output.stop(),
            Effect::Mute => {
                self.muted = true;
                self.output.set_volume(0);
            }
            Effect::Unmute => {
                self.muted = false;
                self.output.set_volume(self.volume);
            }
            Effect::SetVolume(v) => {
                self.volume = v;
                if !self.muted {
                    self.output.set_volume(v);
                }
            }
            Effect::Emit(Event::Item { item_id, phase }) => {
                if phase != ItemPhase::Started {
                    self.audio.retain(|(item, _), _| *item != item_id);
                }
            }
            Effect::Emit(Event::State(_)) => {}
        }
    }
}
