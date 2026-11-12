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
//!
//! Rendered once (2026-11-12): a clip is kept for the engine, voice and
//! rate in force, so a cue said again only plays it. The mute cues are
//! rendered ahead of time, when the worker starts and while it is idle
//! after a new voice or rate, so a mute press is heard at once: Kokoro took
//! 1.2 s for the first "Unmuted." after a pause.
use crate::quiet::Quiet;
use sonara_engine::external::hold;
use sonara_engine::Registry;
use sonara_reader::{Key, ReaderHandle, Value};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::Duration;

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

/// The mute transition cues, rendered ahead of time.
pub const MUTE_CUES: [&str; 3] = ["Muted.", "Unmuted.", "Super muted."];

/// How often an idle worker checks for a new voice or rate.
const IDLE_CHECK: Duration = Duration::from_secs(2);

/// The most clips kept (rate and duck level cues vary).
const MAX_CLIPS: usize = 32;

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

/// What the worker does next.
enum Next {
    Speak(Cue),
    /// Nothing to speak for `IDLE_CHECK`.
    Idle,
    Closed,
}

/// The next cue to speak: a keyed cue with a newer one of the same key
/// waiting is skipped.
fn next(shared: &Shared) -> Next {
    let mut q = lock(&shared.queue);
    loop {
        if q.closed {
            return Next::Closed;
        }
        if let Some(c) = q.cues.pop_front() {
            let superseded = c.key.is_some() && q.cues.iter().any(|later| later.key == c.key);
            if superseded {
                continue;
            }
            return Next::Speak(c);
        }
        let (guard, waited) = shared
            .wake
            .wait_timeout(q, IDLE_CHECK)
            .unwrap_or_else(|p| p.into_inner());
        q = guard;
        if waited.timed_out() && q.cues.is_empty() && !q.closed {
            return Next::Idle;
        }
    }
}

fn run(shared: &Shared, reader: &ReaderHandle, engines: Option<&Registry>, serial: &Mutex<()>) {
    let mut clips = Clips::default();
    if let Some(engines) = engines {
        clips.prerender(reader, engines, serial);
    }
    loop {
        let cue = match next(shared) {
            Next::Speak(cue) => cue,
            Next::Idle => {
                if let Some(engines) = engines {
                    clips.prerender(reader, engines, serial);
                }
                continue;
            }
            Next::Closed => return,
        };
        if let Some(engines) = engines {
            let spoken = clips
                .get(reader, engines, serial, &cue.text, cue.local)
                .and_then(|(samples, rate)| {
                    reader
                        .play_clip(samples.to_vec(), rate)
                        .map_err(|e| e.to_string())
                });
            if let Err(e) = spoken {
                eprintln!("sonarad: cue '{}': {e}", cue.text);
            }
        }
        lock(&shared.subscribers)
            .retain(|s| s.alive.strong_count() > 0 && s.tx.send(cue.text.clone()).is_ok());
    }
}

/// The engine, voice and rate a clip was rendered in, and whether the
/// engine was ready (Kokoro still loading speaks with its fallback, whose
/// clips are rendered again once it is ready).
type Setting = (String, String, u32, bool);

/// A rendered cue: its samples and their rate.
type Clip = (Arc<[i16]>, u32);

/// The clips rendered in the setting in force (module docs).
#[derive(Default)]
struct Clips {
    setting: Option<Setting>,
    /// By text and whether it was spoken locally (#227).
    clips: HashMap<(String, bool), Clip>,
}

impl Clips {
    /// The clip of `text`, rendered now unless it was in this setting.
    fn get(
        &mut self,
        reader: &ReaderHandle,
        engines: &Registry,
        serial: &Mutex<()>,
        text: &str,
        local: bool,
    ) -> Result<Clip, String> {
        let setting = setting(reader, engines)?;
        if self.setting.as_ref() != Some(&setting) {
            self.setting = Some(setting.clone());
            self.clips.clear();
        }
        let key = (text.to_string(), local);
        if let Some(clip) = self.clips.get(&key) {
            return Ok(clip.clone());
        }
        let render = || render(engines, serial, &setting, text);
        let (samples, rate) = if local {
            hold::local(render)?
        } else {
            render()?
        };
        let clip = (Arc::<[i16]>::from(samples), rate);
        if self.clips.len() >= MAX_CLIPS {
            self.clips.clear();
        }
        self.clips.insert(key, clip.clone());
        Ok(clip)
    }

    /// Render the mute cues in the setting in force, unless they are.
    fn prerender(&mut self, reader: &ReaderHandle, engines: &Registry, serial: &Mutex<()>) {
        for text in MUTE_CUES {
            if let Err(e) = self.get(reader, engines, serial, text, true) {
                eprintln!("sonarad: cue '{text}' ahead of time: {e}");
                return;
            }
        }
    }
}

/// The engine, voice and rate in force.
fn setting(reader: &ReaderHandle, engines: &Registry) -> Result<Setting, String> {
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
    let ready = engine.status().readiness == sonara_engine::Readiness::Ready;
    Ok((engine_id, voice, rate, ready))
}

/// Synthesize `text` in `setting` as one mono clip.
fn render(
    engines: &Registry,
    serial: &Mutex<()>,
    (engine_id, voice, rate, _): &Setting,
    text: &str,
) -> Result<(Vec<i16>, u32), String> {
    let engine = engines.get(engine_id).map_err(|e| e.to_string())?;
    let chunks = {
        let _one = lock(serial);
        engine
            .synthesize(text, voice, *rate)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    Ok(mono(&chunks))
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

    /// Cues on a fake engine the test can count.
    fn counted() -> (ReaderHandle, TestOutput, Arc<FakeEngine>, Cues) {
        let registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out.clone()), rx))
                .unwrap();
        let fake = Arc::new(FakeEngine::new());
        let engines = Registry::default();
        engines.register(fake.clone()).unwrap();
        let cues = Cues::new(
            reader.clone(),
            Some(Arc::new(engines)),
            Arc::new(Mutex::new(())),
            quiet(&reader),
        );
        (reader, out, fake, cues)
    }

    /// Wait until `fake` synthesized every mute cue ahead of time.
    fn prerendered(fake: &FakeEngine) {
        let end = Instant::now() + Duration::from_secs(10);
        while !MUTE_CUES
            .iter()
            .all(|c| fake.texts().iter().any(|t| t == c))
        {
            assert!(
                Instant::now() < end,
                "mute cues not rendered: {:?}",
                fake.texts()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A mute press was slow (2026-11-12, 1.2 s on Kokoro after a pause):
    /// its cue was synthesized on every press. The mute cues are rendered
    /// ahead of time, so a press only plays the clip.
    #[test]
    fn a_mute_cue_is_ready_before_the_first_press() {
        let (_reader, out, fake, cues) = counted();
        let heard = cues.subscribe();
        prerendered(&fake);
        let before = fake.syntheses();
        cues.speak_local("Unmuted.");
        assert_eq!(
            heard.recv_timeout(Duration::from_secs(10)).unwrap(),
            "Unmuted."
        );
        assert_eq!(fake.syntheses(), before, "synthesized again on the press");
        assert_eq!(clips(&out), 1);
    }

    #[test]
    fn a_cue_said_again_is_not_synthesized_again() {
        let (_reader, out, fake, cues) = counted();
        let heard = cues.subscribe();
        prerendered(&fake);
        let before = fake.syntheses();
        for _ in 0..3 {
            cues.speak("Paused.", None);
            assert_eq!(
                heard.recv_timeout(Duration::from_secs(10)).unwrap(),
                "Paused."
            );
        }
        assert_eq!(fake.syntheses(), before + 1);
        assert_eq!(clips(&out), 3);
    }

    #[test]
    fn a_new_rate_renders_the_cue_again_in_it() {
        let (reader, out, fake, cues) = counted();
        let heard = cues.subscribe();
        prerendered(&fake);
        cues.speak_local("Muted.");
        heard.recv_timeout(Duration::from_secs(10)).unwrap();
        let before = fake.syntheses();
        reader.set(Key::Rate, Value::Number(300)).unwrap();
        cues.speak_local("Muted.");
        heard.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(fake.syntheses() > before, "the old rate's clip was played");
        let sizes: Vec<usize> = out
            .calls()
            .iter()
            .filter_map(|c| match c {
                OutputCall::PlayClip { samples, .. } => Some(*samples),
                _ => None,
            })
            .collect();
        assert_eq!(sizes.len(), 2);
        assert_ne!(sizes[0], sizes[1], "a faster rate is a shorter clip");
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
