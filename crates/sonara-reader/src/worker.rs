//! The worker thread: owns the reader state machine and carries out its
//! effects (the effect loop). Requests from handles, audio events from the
//! output and finished syntheses all arrive on one inbox, so the reader is
//! touched by this thread only and never waits for an engine.
use crate::settings::{self, Key, Value};
use crate::synth::{Done, Job, Synth};
use crate::{Error, Event, Result};
use sonara_audio::{AudioEvent, Output, PcmChunk};
use sonara_core::reader::{
    Chunking, Control, Effect, Event as CoreEvent, ItemId, ItemPhase, QueueMode, Reader, State,
};
use sonara_engine::{Engine, EngineStatus, InputLimit, Registry, SendMode};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How often the audio forwarder looks for shutdown while the output is
/// quiet.
const FORWARD_POLL: Duration = Duration::from_millis(50);
/// How often the worker reads the engine's status (a model download moves
/// on its own).
const STATUS_POLL: Duration = Duration::from_millis(250);

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
    EngineStatus(Sender<(EngineStatus, u64)>),
    Subscribe(Sender<Receiver<Event>>),
    Clip {
        samples: Vec<i16>,
        sample_rate: u32,
        reply: Sender<()>,
    },
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
    /// The current engine sends whole messages (`ReaderHandle::send_mode`).
    pub whole: Arc<AtomicBool>,
}

/// How the reader cuts items for `engine` (#235).
fn chunking(engine: &dyn Engine) -> Chunking {
    match engine.send_mode() {
        SendMode::Sentence => Chunking::Sentences,
        SendMode::Message => match engine.input_limit() {
            InputLimit::Chars(max) => Chunking::Message { max, bytes: false },
            InputLimit::Bytes(max) => Chunking::Message { max, bytes: true },
        },
    }
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
        open: None,
        pending: VecDeque::new(),
        volume: 100,
        muted: false,
        subscribers: Vec::new(),
        not_ready: None,
        status: EngineStatus::ready(),
        status_changes: 0,
        whole: start.whole,
    };
    thread::Builder::new()
        .name("sonara-reader".into())
        .spawn(move || {
            l.init(start.voice, start.rate, start.volume);
            loop {
                match rx.recv_timeout(STATUS_POLL) {
                    Ok(msg) => {
                        if !l.handle(msg) {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                l.check_status();
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

/// The audio of one chunk: on its way, arriving (a streaming engine,
/// #235), or the engine's answer.
enum Slot {
    Pending,
    Streaming(Vec<PcmChunk>),
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
    /// The chunk playing while its audio still arrives (`Output::play_open`):
    /// its item, chunk and gen, so later pieces are appended and its end
    /// finishes it.
    open: Option<(ItemId, usize, u64)>,
    /// Failures found while carrying out effects, fed to the reader after
    /// the current batch (a failed synthesis surfaces at its play).
    pending: VecDeque<AudioEvent>,
    volume: u8,
    muted: bool,
    subscribers: Vec<Sender<Event>>,
    /// Why the current engine failed its warm-up, told again to each new
    /// subscriber (they can only subscribe once `new` returned).
    not_ready: Option<String>,
    /// The current engine's status as last told to subscribers.
    status: EngineStatus,
    /// How many status changes were told (`Event::EngineStatus::changes`).
    status_changes: u64,
    /// Shared with the handle: the current engine sends whole messages.
    whole: Arc<AtomicBool>,
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
        self.status = self.engine.status();
        let fx = self.reader.set_lookahead(self.engine.lookahead());
        self.run(fx);
        let fx = self.set_chunking();
        self.run(fx);
        self.synth.warm(self.engine.clone());
    }

    /// Cut items as the current engine asks (#235), and tell the handle.
    fn set_chunking(&mut self) -> Vec<Effect> {
        let c = chunking(self.engine.as_ref());
        self.whole.store(c != Chunking::Sentences, Ordering::SeqCst);
        self.reader.set_chunking(c)
    }

    /// Tell subscribers when the current engine's status changed (a model
    /// loaded, a download moved on or failed), with a log line when its
    /// readiness changed (not for every bit of download progress).
    fn check_status(&mut self) {
        let status = self.engine.status();
        if status != self.status {
            let moved = status.readiness != self.status.readiness;
            self.status = status.clone();
            self.status_changes += 1;
            if moved {
                self.broadcast(Event::Log {
                    message: format!("engine '{}' is {status}", self.engine.id()),
                });
            }
            self.broadcast(Event::EngineStatus {
                engine: self.engine.id(),
                status,
                changes: self.status_changes,
            });
        }
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
            Msg::EngineStatus(reply) => {
                // Tell a change first, so the answer and the count agree.
                self.check_status();
                let _ = reply.send((self.status.clone(), self.status_changes));
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
            Msg::Clip {
                samples,
                sample_rate,
                reply,
            } => {
                self.output.play_clip(&samples, sample_rate);
                let _ = reply.send(());
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
                if Arc::ptr_eq(&engine, &self.engine) {
                    return Ok(());
                }
                // The same id in a new instance: the host replaced it (an
                // edited profile). It applies from the next chunk, with the
                // voice kept.
                let same_id = engine.id() == self.engine.id();
                self.engine = engine;
                self.not_ready = None;
                let mut fx = self.reader.set_lookahead(self.engine.lookahead());
                fx.extend(self.set_chunking());
                self.synth.warm(self.engine.clone());
                // A voice of the old engine means nothing to the new one.
                match self.reader.state().voice {
                    Some(v) if !same_id && !settings::offers(self.engine.as_ref(), &v) => {
                        fx.extend(self.reader.set_voice(None))
                    }
                    _ => {}
                }
                fx
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

    /// A piece of a chunk still being made (#235): kept, and played at once
    /// when the reader waits for that chunk (appended when it plays).
    fn part(&mut self, item: ItemId, chunk: usize, pcm: PcmChunk) {
        match self.audio.get_mut(&(item, chunk)) {
            Some(Slot::Pending) => {
                self.audio
                    .insert((item, chunk), Slot::Streaming(vec![pcm.clone()]));
            }
            Some(Slot::Streaming(v)) => v.push(pcm.clone()),
            // The item ended meanwhile.
            _ => return,
        }
        if let Some((i, c, gen)) = self.open {
            if (i, c) == (item, chunk) {
                self.output.append(gen, vec![pcm]);
                return;
            }
        }
        let wanted =
            matches!(&self.waiting, Some(w) if w.item == item && w.chunk == chunk && !w.paused);
        if wanted {
            let w = self.waiting.take().expect("waiting");
            self.load(w.item, w.chunk, w.gen);
            self.drain();
        }
    }

    fn synthesized(&mut self, done: Done) {
        let (item, chunk, result) = match done {
            Done::Part { item, chunk, pcm } => return self.part(item, chunk, pcm),
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
        if !matches!(
            self.audio.get(&(item, chunk)),
            Some(Slot::Pending) | Some(Slot::Streaming(_))
        ) {
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
        // Playing while it arrived: it ends after its last audio.
        if let Some((i, c, gen)) = self.open {
            if (i, c) == (item, chunk) {
                self.open = None;
                self.output.finish(gen);
                return;
            }
        }
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
        self.open = None;
        match self.audio.get(&(item, chunk)) {
            Some(Slot::Ready(Ok(pcm))) => self.output.play(pcm.clone(), item, chunk, gen),
            // Still arriving: play what came, the rest is appended.
            Some(Slot::Streaming(pcm)) => {
                self.output.play_open(pcm.clone(), item, chunk, gen);
                self.open = Some((item, chunk, gen));
            }
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
                Some(w)
                    if matches!(
                        self.audio.get(&(w.item, w.chunk)),
                        Some(Slot::Ready(_)) | Some(Slot::Streaming(_))
                    ) =>
                {
                    self.load(w.item, w.chunk, w.gen)
                }
                Some(mut w) => {
                    w.paused = false;
                    self.waiting = Some(w);
                }
                None => self.output.resume(),
            },
            Effect::StopOutput => {
                self.open = None;
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
                        if self.open.is_some_and(|(i, _, _)| i == *item_id) {
                            self.open = None;
                        }
                        self.audio.retain(|(item, _), _| item != item_id);
                        self.synth.drop_item(*item_id);
                    }
                }
                self.broadcast(e.into());
            }
        }
    }
}
