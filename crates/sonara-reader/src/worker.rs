//! The worker thread: owns the reader state machine and carries out its
//! effects (the effect loop). Requests from handles, audio events from the
//! output and finished syntheses all arrive on one inbox, so the reader is
//! touched by this thread only and never waits for an engine.
use crate::settings::{self, Key, Value};
use crate::synth::{Done, Job, Synth};
use crate::{Error, Event, Result};
use sonara_audio::{AudioEvent, Output, PcmChunk};
use sonara_core::reader::{
    Control, Effect, Event as CoreEvent, ItemId, ItemPhase, QueueMode, Reader, State,
};
use sonara_engine::{Engine, Registry};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How often the audio forwarder looks for shutdown while the output is
/// quiet.
const FORWARD_POLL: Duration = Duration::from_millis(50);

pub(crate) enum Msg {
    Speak {
        text: String,
        mode: QueueMode,
        interrupt: bool,
        label: Option<String>,
        reply: Sender<ItemId>,
    },
    Control(Control, Sender<()>),
    Set {
        key: Key,
        value: Value,
        reply: Sender<Result<()>>,
    },
    Get {
        key: Key,
        reply: Sender<Value>,
    },
    State(Sender<State>),
    Subscribe(Sender<Receiver<Event>>),
    Audio(AudioEvent),
    Synthesized(Done),
    Shutdown,
}

/// Everything the worker starts with (checked by `ReaderHandle::new`).
pub(crate) struct Start {
    pub registry: Arc<Registry>,
    pub engine: Arc<dyn Engine>,
    pub voice: Option<String>,
    pub rate: u32,
    pub volume: u8,
    pub output: Box<dyn Output>,
    pub events: Receiver<AudioEvent>,
}

pub(crate) fn spawn(start: Start, tx: Sender<Msg>, rx: Receiver<Msg>) -> Result<JoinHandle<()>> {
    let to_inbox = tx.clone();
    let synth = Synth::start(move |done| {
        let _ = to_inbox.send(Msg::Synthesized(done));
    })
    .map_err(|e| Error::Start(e.to_string()))?;
    let stop = Arc::new(AtomicBool::new(false));
    let forwarder = {
        let (stop, events) = (stop.clone(), start.events);
        thread::Builder::new()
            .name("sonara-audio-events".into())
            .spawn(move || forward(events, tx, &stop))
            .map_err(|e| Error::Start(e.to_string()))?
    };
    let mut l = Loop {
        reader: Reader::new(),
        registry: start.registry,
        engine: start.engine,
        output: start.output,
        synth,
        audio: HashMap::new(),
        waiting: None,
        pending: VecDeque::new(),
        volume: 100,
        muted: false,
        subscribers: Vec::new(),
        not_ready: None,
    };
    thread::Builder::new()
        .name("sonara-reader".into())
        .spawn(move || {
            l.init(start.voice, start.rate, start.volume);
            while let Ok(msg) = rx.recv() {
                if !l.handle(msg) {
                    break;
                }
            }
            l.close();
            stop.store(true, Ordering::SeqCst);
            let _ = forwarder.join();
        })
        .map_err(|e| Error::Start(e.to_string()))
}

/// Pass the output's events to the inbox until shutdown.
fn forward(events: Receiver<AudioEvent>, tx: Sender<Msg>, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        match events.recv_timeout(FORWARD_POLL) {
            Ok(e) => {
                if tx.send(Msg::Audio(e)).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The audio of one chunk: on its way, or the engine's answer.
enum Slot {
    Pending,
    Ready(std::result::Result<Vec<PcmChunk>, String>),
}

/// A `PlayChunk` whose audio is not in the output yet.
struct Waiting {
    item: ItemId,
    chunk: usize,
    gen: u64,
    /// A `PauseOutput` arrived meanwhile: load it on `ResumeOutput`.
    paused: bool,
}

struct Loop {
    reader: Reader,
    registry: Arc<Registry>,
    engine: Arc<dyn Engine>,
    output: Box<dyn Output>,
    synth: Synth,
    /// Synthesized audio (or the failure) per chunk, until its item ends.
    audio: HashMap<(ItemId, usize), Slot>,
    waiting: Option<Waiting>,
    /// Failures found while carrying out effects, fed to the reader after
    /// the current batch (a failed synthesis surfaces at its play).
    pending: VecDeque<AudioEvent>,
    volume: u8,
    muted: bool,
    subscribers: Vec<Sender<Event>>,
    /// Why the current engine failed its warm-up, told again to each new
    /// subscriber (they can only subscribe once `new` returned).
    not_ready: Option<String>,
}

impl Loop {
    fn init(&mut self, voice: Option<String>, rate: u32, volume: u8) {
        let fx = self.reader.set_voice(voice);
        self.run(fx);
        let fx = self.reader.set_rate(rate);
        self.run(fx);
        self.volume = self.reader.state().volume;
        let fx = self.reader.set_volume(volume);
        self.run(fx);
        self.synth.warm(self.engine.clone());
    }

    /// Handle one message; false ends the loop.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Speak {
                text,
                mode,
                interrupt,
                label,
                reply,
            } => {
                let (id, fx) = self.reader.speak(&text, mode, interrupt, label);
                self.run(fx);
                let _ = reply.send(id);
            }
            Msg::Control(c, reply) => {
                let fx = self.reader.control(c);
                self.run(fx);
                let _ = reply.send(());
            }
            Msg::Set { key, value, reply } => {
                let r = self.set(key, value);
                let _ = reply.send(r);
            }
            Msg::Get { key, reply } => {
                let _ = reply.send(self.get(key));
            }
            Msg::State(reply) => {
                let _ = reply.send(self.reader.state());
            }
            Msg::Subscribe(reply) => {
                let (tx, rx) = channel();
                if let Some(message) = &self.not_ready {
                    let _ = tx.send(Event::Log {
                        message: message.clone(),
                    });
                }
                self.subscribers.push(tx);
                let _ = reply.send(rx);
            }
            Msg::Audio(e) => self.audio_event(e),
            Msg::Synthesized(done) => self.synthesized(done),
            Msg::Shutdown => return false,
        }
        true
    }

    /// Stop speech (subscribers see the items end), cancel synthesis and
    /// release the output and the subscribers.
    fn close(&mut self) {
        let fx = self.reader.control(Control::Stop);
        self.run(fx);
        self.synth.shutdown();
        self.subscribers.clear();
    }

    fn set(&mut self, key: Key, value: Value) -> Result<()> {
        let fx = match key {
            Key::Volume => {
                let v = settings::check_volume(settings::number(key, &value)?)?;
                self.reader.set_volume(v)
            }
            Key::Rate => {
                let v = settings::check_rate(settings::number(key, &value)?)?;
                self.reader.set_rate(v)
            }
            Key::Voice => {
                let voice = match &value {
                    Value::Null => None,
                    Value::Text(t) => settings::resolve_voice(self.engine.as_ref(), Some(t))?,
                    other => {
                        return Err(Error::BadValue {
                            key,
                            reason: format!("expected a voice name or null, got {other:?}"),
                        })
                    }
                };
                self.reader.set_voice(voice)
            }
            Key::Engine => {
                let Value::Text(id) = &value else {
                    return Err(Error::BadValue {
                        key,
                        reason: format!("expected an engine id, got {value:?}"),
                    });
                };
                let engine = self.registry.get(id)?;
                if engine.id() == self.engine.id() {
                    return Ok(());
                }
                self.engine = engine;
                self.not_ready = None;
                self.synth.warm(self.engine.clone());
                // A voice of the old engine means nothing to the new one.
                match self.reader.state().voice {
                    Some(v) if !settings::offers(self.engine.as_ref(), &v) => {
                        self.reader.set_voice(None)
                    }
                    _ => Vec::new(),
                }
            }
        };
        self.run(fx);
        Ok(())
    }

    fn get(&self, key: Key) -> Value {
        let s = self.reader.state();
        match key {
            Key::Volume => Value::Number(s.volume as u64),
            Key::Rate => Value::Number(s.rate as u64),
            Key::Voice => s.voice.map_or(Value::Null, Value::Text),
            Key::Engine => Value::Text(self.engine.id().as_str().to_string()),
        }
    }

    fn broadcast(&mut self, e: Event) {
        self.subscribers.retain(|s| s.send(e.clone()).is_ok());
    }

    fn audio_event(&mut self, e: AudioEvent) {
        let fx = self.reader.on_audio(e);
        self.run(fx);
    }

    fn synthesized(&mut self, done: Done) {
        let (item, chunk, result) = match done {
            Done::Chunk {
                item,
                chunk,
                result,
            } => (item, chunk, result),
            Done::WarmFailed { engine, message } => {
                if engine == self.engine.id() {
                    self.not_ready = Some(message.clone());
                }
                self.broadcast(Event::Log { message });
                return;
            }
        };
        // Not pending: the item ended meanwhile (a cancelled job lands here).
        if !matches!(self.audio.get(&(item, chunk)), Some(Slot::Pending)) {
            return;
        }
        let result = result.map_err(|e| e.to_string());
        if let Err(reason) = &result {
            self.broadcast(Event::Log {
                message: format!(
                    "synthesis failed for item {} chunk {chunk}: {reason}",
                    item.0
                ),
            });
        }
        self.audio.insert((item, chunk), Slot::Ready(result));
        let ready = matches!(&self.waiting, Some(w) if w.item == item && w.chunk == chunk);
        if ready {
            let w = self.waiting.take().expect("waiting");
            if w.paused {
                // Loaded on ResumeOutput; a failure is reported at once.
                if let Some(Slot::Ready(Ok(_))) = self.audio.get(&(item, chunk)) {
                    self.waiting = Some(w);
                    return;
                }
            }
            self.load(w.item, w.chunk, w.gen);
            self.drain();
        }
    }

    fn run(&mut self, fx: Vec<Effect>) {
        for f in fx {
            self.apply(f);
        }
        self.drain();
    }

    fn drain(&mut self) {
        while let Some(e) = self.pending.pop_front() {
            let fx = self.reader.on_audio(e);
            for f in fx {
                self.apply(f);
            }
        }
    }

    /// Hand a chunk's audio to the output, or report why there is none.
    fn load(&mut self, item: ItemId, chunk: usize, gen: u64) {
        match self.audio.get(&(item, chunk)) {
            Some(Slot::Ready(Ok(pcm))) => self.output.play(pcm.clone(), item, chunk, gen),
            Some(Slot::Ready(Err(reason))) => self.pending.push_back(AudioEvent::Failed {
                gen,
                reason: reason.clone(),
            }),
            Some(Slot::Pending) => {
                self.waiting = Some(Waiting {
                    item,
                    chunk,
                    gen,
                    paused: false,
                });
                self.synth.promote(item, chunk);
            }
            None => self.pending.push_back(AudioEvent::Failed {
                gen,
                reason: "chunk was never synthesized".into(),
            }),
        }
    }

    fn apply(&mut self, f: Effect) {
        match f {
            Effect::Synthesize { item, chunk, text } => {
                let state = self.reader.state();
                self.audio.insert((item, chunk), Slot::Pending);
                self.synth.push_job(Job {
                    item,
                    chunk,
                    text,
                    voice: state.voice.unwrap_or_default(),
                    rate: state.rate,
                    engine: self.engine.clone(),
                });
            }
            Effect::PlayChunk { item, chunk, gen } => {
                self.waiting = None;
                self.load(item, chunk, gen);
            }
            Effect::PauseOutput => match &mut self.waiting {
                Some(w) => w.paused = true,
                None => self.output.pause(),
            },
            Effect::ResumeOutput => match self.waiting.take() {
                Some(w) if matches!(self.audio.get(&(w.item, w.chunk)), Some(Slot::Ready(_))) => {
                    self.load(w.item, w.chunk, w.gen)
                }
                Some(mut w) => {
                    w.paused = false;
                    self.waiting = Some(w);
                }
                None => self.output.resume(),
            },
            Effect::StopOutput => {
                if self.waiting.take().is_none() {
                    self.output.stop();
                }
            }
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
            Effect::Emit(e) => {
                if let CoreEvent::Item { item_id, phase } = &e {
                    if *phase != ItemPhase::Started {
                        self.audio.retain(|(item, _), _| item != item_id);
                        self.synth.drop_item(*item_id);
                    }
                }
                self.broadcast(e.into());
            }
        }
    }
}
