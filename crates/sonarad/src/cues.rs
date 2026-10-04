//! Spoken control cues of the `system` extension (#197): the short
//! confirmations the Python plugin spoke after a hotkey or a setting
//! change ("Paused.", "Muted.", "Rate 250.", "Audio ducking.", ...).
//!
//! A cue is synthesized on the extension's own engines (those of the voice
//! previews), in the voice and at the rate in force, and played as a clip
//! mixed over whatever is being read (`ReaderHandle::play_clip`, like an
//! earcon). So a cue is fast and never touches the queue: it is heard
//! while the reader is paused ("Paused.") and whatever the agent's mute
//! level ("Muted.", "Super muted."), as the Python cues were exempt from
//! pause and mute; a muted reader (core `mute`) plays it silently.
//!
//! Muted (#227): a cue never reaches an external engine while Sonara is
//! muted. The mute transition cues ("Muted.", "Super muted.", "Unmuted.",
//! `speak_local`) and any cue queued while muted are synthesized inside
//! `hold::local`, so an external engine speaks them with its local
//! fallback (Kokoro, else OneCore).
//!
//! Cues run on one worker thread, in order. A cue with a key (`rate`,
//! `duck_level`) is dropped when a newer cue with the same key is waiting,
//! so a burst of presses or a dragged slider speaks only the last value.
//! Every cue spoken is reported to `subscribe`rs (the `cue` event).
use crate::quiet::Quiet;
use sonara_engine::external::hold;
use sonara_engine::Registry;
use sonara_reader::{Key, ReaderHandle, Value};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};

/// One cue to speak.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cue {
    text: String,
    key: Option<&'static str>,
    /// Spoken with the local voice, never an external engine.
    local: bool,
}

/// Cues as they are spoken (`Cues::subscribe`). Dropping it unsubscribes.
pub struct CueStream {
    rx: Receiver<String>,
    _alive: Arc<()>,
}

impl std::ops::Deref for CueStream {
    type Target = Receiver<String>;

    fn deref(&self) -> &Receiver<String> {
        &self.rx
    }
}

struct Subscriber {
    tx: Sender<String>,
    alive: Weak<()>,
}

struct Queue {
    cues: VecDeque<Cue>,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    subscribers: Mutex<Vec<Subscriber>>,
    quiet: Quiet,
}

/// The cue path (module docs).
pub struct Cues {
    shared: Arc<Shared>,
}

/// The cue for a mute level (the agent's three-level cycle).
pub fn mute_level_cue(level: u64) -> &'static str {
    match level {
        0 => "Unmuted.",
        1 => "Muted.",
        _ => "Super muted.",
    }
}

/// The cue for an audio mode (Python `audio.py`).
pub fn audio_mode_cue(mode: &str) -> Option<&'static str> {
    match mode {
        "off" => Some("Audio off."),
        "duck" => Some("Audio ducking."),
        "pause" => Some("Media pause."),
        _ => None,
    }
}

pub fn rate_cue(rate: u64) -> String {
    format!("Rate {rate}.")
}

pub fn duck_level_cue(level: u64) -> String {
    format!("Duck level {level} percent.")
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Cues {
    /// Start the worker. `engines` synthesizes (the preview engines; `None`:
    /// cues are only reported, never heard), `serial` is held while
    /// synthesizing (shared with the previews), `reader` plays the clips
    /// and gives the engine, voice and rate in force; while `quiet` holds,
    /// cues are local.
    pub fn new(
        reader: ReaderHandle,
        engines: Option<Arc<Registry>>,
        serial: Arc<Mutex<()>>,
        quiet: Quiet,
    ) -> Cues {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                cues: VecDeque::new(),
                closed: false,
            }),
            wake: Condvar::new(),
            subscribers: Mutex::new(Vec::new()),
            quiet,
        });
        let worker = shared.clone();
        let _ = std::thread::Builder::new()
            .name("sonarad-cues".into())
            .spawn(move || run(&worker, &reader, engines.as_deref(), &serial));
        Cues { shared }
    }

    /// Speak `text` once the cues before it are spoken. `key`: drop it if a
    /// newer cue with the same key is waiting by then.
    /// While Sonara is muted it is spoken locally (#227).
    pub fn speak(&self, text: &str, key: Option<&'static str>) {
        let local = self.shared.quiet.is_held();
        self.push(text, key, local);
    }

    /// Speak a mute transition's cue ("Muted.", "Unmuted.") with the local
    /// voice, never an external engine (#227).
    pub fn speak_local(&self, text: &str) {
        self.push(text, None, true);
    }

    fn push(&self, text: &str, key: Option<&'static str>, local: bool) {
        let mut q = lock(&self.shared.queue);
        if q.closed {
            return;
        }
        q.cues.push_back(Cue {
            text: text.to_string(),
            key,
            local,
        });
        self.shared.wake.notify_one();
    }

    /// Every cue spoken from now on.
    pub fn subscribe(&self) -> CueStream {
        let (tx, rx) = channel();
        let alive = Arc::new(());
        let mut subs = lock(&self.shared.subscribers);
        subs.retain(|s| s.alive.strong_count() > 0);
        subs.push(Subscriber {
            tx,
            alive: Arc::downgrade(&alive),
        });
        CueStream { rx, _alive: alive }
    }

    /// Stop the worker (the cues still waiting are dropped).
    pub fn shutdown(&self) {
        let mut q = lock(&self.shared.queue);
        q.closed = true;
        q.cues.clear();
        self.shared.wake.notify_all();
    }
}

impl Drop for Cues {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The next cue to speak: a keyed cue with a newer one of the same key
/// waiting is skipped. `None` once closed.
fn next(shared: &Shared) -> Option<Cue> {
    let mut q = lock(&shared.queue);
    loop {
        if q.closed {
            return None;
        }
        if let Some(c) = q.cues.pop_front() {
            let superseded = c.key.is_some() && q.cues.iter().any(|later| later.key == c.key);
            if superseded {
                continue;
            }
            return Some(c);
        }
        q = shared.wake.wait(q).unwrap_or_else(|p| p.into_inner());
    }
}

fn run(shared: &Shared, reader: &ReaderHandle, engines: Option<&Registry>, serial: &Mutex<()>) {
    while let Some(cue) = next(shared) {
        if let Some(engines) = engines {
            let spoken = if cue.local {
                hold::local(|| say(reader, engines, serial, &cue.text))
            } else {
                say(reader, engines, serial, &cue.text)
            };
            if let Err(e) = spoken {
                eprintln!("sonarad: cue '{}': {e}", cue.text);
            }
        }
        lock(&shared.subscribers)
            .retain(|s| s.alive.strong_count() > 0 && s.tx.send(cue.text.clone()).is_ok());
    }
}

/// Synthesize `text` in the voice and at the rate in force and play it as
/// a clip.
fn say(
    reader: &ReaderHandle,
    engines: &Registry,
    serial: &Mutex<()>,
    text: &str,
) -> Result<(), String> {
    let engine_id = match reader.get(Key::Engine).map_err(|e| e.to_string())? {
        Value::Text(t) => t,
        _ => return Err("no engine".into()),
    };
    let engine = engines.get(&engine_id).map_err(|e| e.to_string())?;
    let voice = match reader.get(Key::Voice).map_err(|e| e.to_string())? {
        Value::Text(t) => engine
            .voices()
            .into_iter()
            .find(|v| v.id == t || v.name == t)
            .map(|v| v.id)
            .unwrap_or_default(),
        _ => String::new(),
    };
    let rate = match reader.get(Key::Rate).map_err(|e| e.to_string())? {
        Value::Number(n) => n as u32,
        _ => 200,
    };
    let chunks = {
        let _one = lock(serial);
        engine
            .synthesize(text, &voice, rate)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    let (samples, sample_rate) = mono(&chunks);
    reader
        .play_clip(samples, sample_rate)
        .map_err(|e| e.to_string())
}

/// The chunks as one mono clip (`play_clip` takes one channel).
pub fn mono(chunks: &[sonara_engine::PcmChunk]) -> (Vec<i16>, u32) {
    let sample_rate = chunks.first().map(|c| c.sample_rate).unwrap_or(16_000);
    let mut samples: Vec<i16> = Vec::new();
    for c in chunks {
        if c.channels <= 1 {
            samples.extend_from_slice(&c.samples);
        } else {
            let n = usize::from(c.channels);
            samples.extend(
                c.samples.chunks(n).map(|f| {
                    (f.iter().map(|&x| i32::from(x)).sum::<i32>() / f.len() as i32) as i16
                }),
            );
        }
    }
    (samples, sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonara_audio::{OutputCall, TestOutput};
    use sonara_engine::fake::FakeEngine;
    use sonara_reader::Config;
    use std::time::{Duration, Instant};

    fn reader() -> (ReaderHandle, TestOutput, Arc<Registry>) {
        let registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out.clone()), rx))
                .unwrap();
        let engines = Registry::default();
        engines.register(Arc::new(FakeEngine::new())).unwrap();
        (reader, out, Arc::new(engines))
    }

    fn quiet(reader: &ReaderHandle) -> Quiet {
        Quiet::new(reader.clone(), Arc::new(std::sync::OnceLock::new()))
    }

    fn clips(out: &TestOutput) -> usize {
        out.calls()
            .iter()
            .filter(|c| matches!(c, OutputCall::PlayClip { .. }))
            .count()
    }

    #[test]
    fn a_cue_is_played_as_a_clip_and_reported() {
        let (reader, out, engines) = reader();
        let cues = Cues::new(
            reader.clone(),
            Some(engines),
            Arc::new(Mutex::new(())),
            quiet(&reader),
        );
        let heard = cues.subscribe();
        cues.speak("Paused.", None);
        assert_eq!(
            heard.recv_timeout(Duration::from_secs(10)).unwrap(),
            "Paused."
        );
        assert_eq!(clips(&out), 1);
        assert!(
            reader.state().unwrap().now_playing.is_none(),
            "queue untouched"
        );
    }

    #[test]
    fn a_keyed_cue_waiting_behind_a_newer_one_is_dropped() {
        let reader = reader().0;
        let cues = Cues::new(
            reader.clone(),
            None,
            Arc::new(Mutex::new(())),
            quiet(&reader),
        );
        let heard = cues.subscribe();
        // Hold the worker: enqueue while it is blocked on the queue lock.
        {
            let mut q = lock(&cues.shared.queue);
            for t in ["Rate 225.", "Muted.", "Rate 250.", "Rate 275."] {
                let key = t.starts_with("Rate").then_some("rate");
                q.cues.push_back(Cue {
                    text: t.into(),
                    key,
                    local: false,
                });
            }
            cues.shared.wake.notify_one();
        }
        let mut got = Vec::new();
        let end = Instant::now() + Duration::from_secs(10);
        while got.len() < 2 && Instant::now() < end {
            if let Ok(t) = heard.recv_timeout(Duration::from_millis(100)) {
                got.push(t);
            }
        }
        assert_eq!(got, ["Muted.", "Rate 275."]);
    }

    #[test]
    fn cue_texts() {
        assert_eq!(mute_level_cue(0), "Unmuted.");
        assert_eq!(mute_level_cue(1), "Muted.");
        assert_eq!(mute_level_cue(2), "Super muted.");
        assert_eq!(audio_mode_cue("duck"), Some("Audio ducking."));
        assert_eq!(audio_mode_cue("pause"), Some("Media pause."));
        assert_eq!(audio_mode_cue("off"), Some("Audio off."));
        assert_eq!(rate_cue(250), "Rate 250.");
        assert_eq!(duck_level_cue(30), "Duck level 30 percent.");
    }

    #[test]
    fn subscribers_that_went_away_are_pruned_and_shutdown_stops() {
        let reader = reader().0;
        let cues = Cues::new(
            reader.clone(),
            None,
            Arc::new(Mutex::new(())),
            quiet(&reader),
        );
        drop(cues.subscribe());
        let _kept = cues.subscribe();
        assert_eq!(lock(&cues.shared.subscribers).len(), 1);
        cues.shutdown();
        cues.speak("Late.", None);
        assert!(lock(&cues.shared.queue).cues.is_empty());
    }
}
