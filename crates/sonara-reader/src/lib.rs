//! Sonara L1 facade: the reader as an in-process library.
//!
//! `ReaderHandle::new` starts one worker thread that owns the reader state
//! machine (`sonara_core::reader::Reader`) and carries out its effects with
//! an engine and an audio output; the handle sends it requests. L2+ crates
//! and `sonarad` use only this facade.
//!
//! - Synthesis runs on its own thread, never on the control path: a slow
//!   engine never delays `control` or `speak`. The state machine decides
//!   what to synthesize (the playing chunk and one ahead); a chunk whose
//!   item ends is dropped from the queue, or cancelled while in flight.
//! - A failed synthesis is reported to the reader as `AudioEvent::Failed`
//!   under the `gen` of the chunk's `PlayChunk` (the rule documented on
//!   `Effect::Synthesize`), so the chunk is skipped, and to subscribers as
//!   `Event::Log`.
//! - `subscribe` gives every subscriber every event from then on. Sending
//!   never waits (the channels are unbounded), so a slow subscriber never
//!   blocks the reader; a dropped one is forgotten on the next event. A host
//!   that relays events to clients (`sonarad`) bounds or drains its own
//!   per-client queues.
//! - The handle is `Clone + Send + Sync`. Each call is answered by the
//!   worker after its effects were carried out, so calls from one thread
//!   apply in order. `shutdown` (or dropping the last handle) stops speech,
//!   cancels synthesis and joins the threads; later calls return
//!   `Error::Closed`.
mod error;
mod settings;
mod synth;
mod worker;

pub use error::{Error, Result};
pub use settings::{Key, Value, RATE_MAX, RATE_MIN, VOLUME_MAX};

pub use sonara_audio::{AudioEvent, Output};
pub use sonara_core::reader::{Chunking, Control, ItemId, ItemPhase, NowPlaying, QueueMode, State};
pub use sonara_engine::{Engine, EngineId, EngineStatus, Readiness, Registry, SendMode, Voice};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use worker::Msg;

/// What subscribers are told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The reader state changed (`seq` strictly increasing).
    State(State),
    /// An item started or ended.
    Item { item_id: ItemId, phase: ItemPhase },
    /// Something worth a log line: a failed synthesis, an engine that is not
    /// ready.
    Log { message: String },
    /// The current engine's readiness changed (Kokoro loading or
    /// downloading its model, speaking with its fallback meanwhile). Polled
    /// about four times a second; engines that are always ready never send
    /// it. `changes` counts the status changes this reader has told so
    /// far (the same for every subscriber), so a host can give each a
    /// `seq` that is the same on every stream.
    EngineStatus {
        engine: EngineId,
        status: EngineStatus,
        changes: u64,
    },
}

impl From<sonara_core::reader::Event> for Event {
    fn from(e: sonara_core::reader::Event) -> Self {
        match e {
            sonara_core::reader::Event::State(s) => Event::State(s),
            sonara_core::reader::Event::Item { item_id, phase } => Event::Item { item_id, phase },
        }
    }
}

/// How to build a reader.
pub struct Config {
    /// The engines this host allows (R6 is enforced by the registry). It
    /// may be shared with the host, which can add and remove engines while
    /// the reader runs (external engine profiles).
    pub registry: Arc<Registry>,
    /// Engine id to start with; `None` takes the first one registered.
    pub engine: Option<String>,
    /// Voice id or name of that engine; `None` is the engine default.
    pub voice: Option<String>,
    /// Words per minute, `RATE_MIN..=RATE_MAX`.
    pub rate: u32,
    /// Percent, `0..=VOLUME_MAX`.
    pub volume: u8,
    /// The audio output and its event channel; `None` opens the default
    /// device (`RodioOutput`).
    pub output: Option<(Box<dyn Output>, Receiver<AudioEvent>)>,
}

impl Config {
    /// The defaults of the Python reader: first engine, its default voice,
    /// rate 200, volume 100, the default audio device.
    pub fn new(registry: impl Into<Arc<Registry>>) -> Self {
        Config {
            registry: registry.into(),
            engine: None,
            voice: None,
            rate: 200,
            volume: 100,
            output: None,
        }
    }

    /// Use this output instead of the default device (tests use
    /// `TestOutput`).
    pub fn with_output(mut self, output: Box<dyn Output>, events: Receiver<AudioEvent>) -> Self {
        self.output = Some((output, events));
        self
    }
}

/// The engines this build can offer, under the default licence policy
/// (permissive and OS engines): OneCore on Windows with feature `onecore`.
pub fn default_registry() -> Registry {
    registry_with(&[
        sonara_engine::LicenseClass::Permissive,
        sonara_engine::LicenseClass::Os,
    ])
}

/// The engines this build can offer, in a registry that allows `allowed`
/// (a host that takes external engines adds `LicenseClass::External`).
/// OneCore is registered when the OS class is allowed.
pub fn registry_with(allowed: &[sonara_engine::LicenseClass]) -> Registry {
    let registry = Registry::new(allowed);
    #[cfg(all(windows, feature = "onecore"))]
    if registry.allows(sonara_engine::LicenseClass::Os) {
        registry
            .register(Arc::new(sonara_engine::onecore::OneCore::new()))
            .expect("the OS class is allowed");
    }
    registry
}

struct Shared {
    tx: Sender<Msg>,
    registry: Arc<Registry>,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// The current engine sends whole messages (set by the worker).
    whole: Arc<AtomicBool>,
}

impl Shared {
    /// Joins while holding the lock, so a concurrent caller returns only
    /// once the threads have stopped.
    fn shutdown(&self) {
        let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(worker) = worker.take() {
            let _ = self.tx.send(Msg::Shutdown);
            let _ = worker.join();
        }
    }
}

impl Shared {
    fn is_closed(&self) -> bool {
        self.worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none()
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A running reader. Clones share it; it shuts down with the last clone or
/// on `shutdown`.
#[derive(Clone)]
pub struct ReaderHandle {
    shared: Arc<Shared>,
}

impl ReaderHandle {
    /// Check the config and start the reader. Fails on an unknown engine or
    /// voice, an out-of-range rate or volume, or an empty registry.
    pub fn new(config: Config) -> Result<ReaderHandle> {
        let registry = config.registry;
        let engine = match &config.engine {
            Some(id) => registry.get(id)?,
            None => {
                let first = registry.ids().first().copied().ok_or(Error::NoEngine)?;
                registry.get(first.as_str())?
            }
        };
        let voice = settings::resolve_voice(engine.as_ref(), config.voice.as_deref())?;
        settings::check_rate(config.rate as u64)?;
        settings::check_volume(config.volume as u64)?;
        let (output, events) = match config.output {
            Some(o) => o,
            None => {
                let (out, events) = sonara_audio::RodioOutput::new();
                (Box::new(out) as Box<dyn Output>, events)
            }
        };
        let (tx, rx) = channel();
        let whole = Arc::new(AtomicBool::new(false));
        let start = worker::Start {
            whole: whole.clone(),
            registry: registry.clone(),
            engine,
            voice,
            rate: config.rate,
            volume: config.volume,
            output,
            events,
        };
        let worker = worker::spawn(start, tx.clone(), rx)?;
        Ok(ReaderHandle {
            shared: Arc::new(Shared {
                tx,
                registry,
                worker: Mutex::new(Some(worker)),
                whole,
            }),
        })
    }

    fn call<T>(&self, make: impl FnOnce(Sender<T>) -> Msg) -> Result<T> {
        let (reply, rx) = channel();
        self.shared
            .tx
            .send(make(reply))
            .map_err(|_| Error::Closed)?;
        rx.recv().map_err(|_| Error::Closed)
    }

    /// Add `text` as one item (see `sonara_core::reader` for `mode` and
    /// `interrupt`). Returns its id.
    pub fn speak(
        &self,
        text: &str,
        mode: QueueMode,
        interrupt: bool,
        label: Option<String>,
    ) -> Result<ItemId> {
        let text = text.to_string();
        self.call(|reply| Msg::Speak {
            text,
            mode,
            interrupt,
            label,
            reply,
        })
    }

    /// A playback control; returns once the reader has carried it out.
    pub fn control(&self, c: Control) -> Result<()> {
        self.call(|reply| Msg::Control(c, reply))
    }

    /// Change a setting (`volume` 0..=100, `rate` 100..=400 words per
    /// minute, `voice` id or name of the current engine or `Null` for its
    /// default, `engine` id). A rate, voice or engine applies to chunks
    /// synthesized from now on.
    pub fn set(&self, key: Key, value: Value) -> Result<()> {
        self.call(|reply| Msg::Set { key, value, reply })?
    }

    /// Read a setting: numbers for `volume` and `rate`, the voice id (or
    /// `Null` for the default) and the engine id as text.
    pub fn get(&self, key: Key) -> Result<Value> {
        self.call(|reply| Msg::Get { key, reply })
    }

    /// The current state snapshot.
    pub fn state(&self) -> Result<State> {
        self.call(Msg::State)
    }

    /// How the current engine takes text (#235): `Message` when it reads
    /// each item as one text (`Engine::send_mode`), so L3 joins what it
    /// releases at once into one item. Cheap: no round trip to the worker,
    /// which updates it at start and on every engine change.
    pub fn send_mode(&self) -> SendMode {
        if self.shared.whole.load(Ordering::SeqCst) {
            SendMode::Message
        } else {
            SendMode::Sentence
        }
    }

    /// The current engine's readiness (spec 4.1 `engine_status`).
    pub fn engine_status(&self) -> Result<EngineStatus> {
        Ok(self.engine_status_changes()?.0)
    }

    /// The current engine's readiness and how many status changes the
    /// reader has told (`Event::EngineStatus::changes`).
    pub fn engine_status_changes(&self) -> Result<(EngineStatus, u64)> {
        self.call(Msg::EngineStatus)
    }

    /// The engines this reader can switch to (shared: a host may register
    /// more while it runs).
    pub fn registry(&self) -> Arc<Registry> {
        self.shared.registry.clone()
    }

    /// Fetch an engine's voice list from its source (an external engine
    /// asks its provider; it may block up to that engine's timeout).
    pub fn refresh_voices(&self, engine: &str) -> Result<Vec<Voice>> {
        if self.shared.is_closed() {
            return Err(Error::Closed);
        }
        Ok(self.shared.registry.get(engine)?.refresh_voices()?)
    }

    /// Voices of one engine, or of every registered engine.
    pub fn voices(&self, engine: Option<&str>) -> Result<Vec<Voice>> {
        if self.shared.is_closed() {
            return Err(Error::Closed);
        }
        match engine {
            Some(id) => Ok(self.shared.registry.get(id)?.voices()),
            None => Ok(self.shared.registry.voices()),
        }
    }

    /// Every event from now on. Dropping the receiver unsubscribes. If the
    /// current engine failed its warm-up, the first event is that `Log`.
    pub fn subscribe(&self) -> Result<Receiver<Event>> {
        self.call(Msg::Subscribe)
    }

    /// Play a short mono clip (an earcon, L3) over the speech through the
    /// output's mixer: it neither pauses nor cuts the item playing and does
    /// not touch the reader state. The output volume applies, so a reader
    /// muted with `Control::Mute` plays it silently.
    pub fn play_clip(&self, samples: Vec<i16>, sample_rate: u32) -> Result<()> {
        self.call(|reply| Msg::Clip {
            samples,
            sample_rate,
            reply,
        })
    }

    /// Stop speech, cancel synthesis and stop the threads. Subscribers see
    /// the final events, then their channel closes. Idempotent.
    pub fn shutdown(&self) {
        self.shared.shutdown();
    }
}
