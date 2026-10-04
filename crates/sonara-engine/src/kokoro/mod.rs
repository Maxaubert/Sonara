//! The Kokoro engine (feature `kokoro`): Kokoro-82M v1.0 (Apache-2.0
//! weights) on Microsoft's CPU ONNX Runtime, with phonemes from a vendored
//! misaki (no espeak; spec R2). Licence class `Permissive`.
//!
//! - **Model files** come from `download::Manager`: present, pre-seeded by
//!   the host, or downloaded in the background on first use.
//! - **Until Kokoro is ready** (downloading, a failure waiting for its
//!   retry, no ONNX Runtime), `synthesize` speaks with the host's fallback
//!   engine (OneCore in `sonarad`) at once, and `status` says why. Loading
//!   a model that is on disk takes about a second; a synthesis waits for it
//!   rather than switching voices.
//! - **A fallback that cannot speak** (its warm-up fails or it lists no
//!   voices, as OneCore on a PC without voices for new programs) counts as
//!   none: a synthesis then waits for Kokoro while it is downloading or
//!   loading (at most `NO_FALLBACK_WAIT`, ended by `cancel`), so a fresh
//!   install drops no speech.
//! - **Synthesis** runs the text rules (`text`), G2P (`phonemes`), then one
//!   model call per sentence (`phonemes::batches`); each batch is a
//!   `PcmChunk` (24 kHz mono), silence-trimmed and loudness-normalized like
//!   the Python reader (#81).
//! - **Rate**: Sonara words per minute map to Kokoro's speed as
//!   `clamp(wpm / 200, 0.5, 2.0)` (`speed`), as the Python reader.
pub mod download;
mod fallback;
pub mod model;
pub mod phonemes;
pub mod text;
pub mod vocab;
pub mod voices;

use crate::{
    Engine, EngineId, EngineStatus, Error, LicenseClass, PcmChunk, PcmStream, Readiness, Result,
    Voice,
};
use download::{Backoff, Manager, ModelFile, Phase};
use model::Acoustic;
use phonemes::Phonemizer;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use voices::{Styles, DEFAULT_VOICE, ENGLISH_VOICES};

pub const ID: EngineId = EngineId("kokoro");
pub use model::SAMPLE_RATE;

/// Sonara's default rate (words per minute) is Kokoro speed 1.0.
pub const BASELINE_WPM: f32 = 200.0;
pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 2.0;

/// Kokoro speed for a Sonara rate (the Python `rate_to_speed`).
pub fn speed(wpm: u32) -> f32 {
    (wpm as f32 / BASELINE_WPM).clamp(MIN_SPEED, MAX_SPEED)
}

/// Builds the acoustic model from the model file (tests pass a fake).
pub type Loader = Arc<dyn Fn(&Path) -> Result<Box<dyn Acoustic>> + Send + Sync>;

/// How long a synthesis waits for a model that is loading.
const LOAD_WAIT: Duration = Duration::from_secs(30);

/// How long a synthesis waits for Kokoro when no fallback can speak.
pub const NO_FALLBACK_WAIT: Duration = Duration::from_secs(5 * 60);

/// How to build the engine.
pub struct Config {
    /// `<home>\models\kokoro\v1.0`.
    pub model_dir: PathBuf,
    /// The files to have there (`download::pinned_files`).
    pub files: Vec<ModelFile>,
    /// `onnxruntime.dll` (next to `sonarad.exe`).
    pub runtime: PathBuf,
    /// Download missing files (false: use only what is on disk).
    pub download: bool,
    pub backoff: Backoff,
    /// What speaks while Kokoro is not ready.
    pub fallback: Option<Arc<dyn Engine>>,
    /// `None`: ONNX Runtime from `runtime`.
    pub loader: Option<Loader>,
}

impl Config {
    /// The pinned files from `download::BASE_URL`, downloads on, default
    /// backoff, no fallback.
    pub fn new(model_dir: PathBuf, runtime: PathBuf) -> Self {
        Config {
            model_dir,
            files: download::pinned_files(download::BASE_URL),
            runtime,
            download: true,
            backoff: Backoff::default(),
            fallback: None,
            loader: None,
        }
    }
}

/// The phonemizer, built once per process (it decodes the lexicons) and
/// shared by every engine instance.
fn phonemizer() -> Arc<Phonemizer> {
    static P: OnceLock<Arc<Phonemizer>> = OnceLock::new();
    P.get_or_init(|| Arc::new(Phonemizer::new())).clone()
}

struct Loaded {
    model: Mutex<Box<dyn Acoustic>>,
    styles: Styles,
    phonemizer: Arc<Phonemizer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    /// Verifying or downloading the files.
    Fetching,
    /// Loading the model into memory.
    Loading,
}

struct Progress {
    stage: Stage,
    loaded: Option<Arc<Loaded>>,
    /// Kokoro cannot run in this install (no ONNX Runtime): for good.
    unavailable: Option<String>,
    /// The last load failure and when to try again.
    load_failed: Option<(String, Instant, u32)>,
}

struct Inner {
    manager: Manager,
    runtime: PathBuf,
    download: bool,
    backoff: Backoff,
    fallback: Option<Arc<dyn Engine>>,
    /// Whether the fallback can speak here (warmed and has voices), found
    /// out once.
    fallback_ok: OnceLock<bool>,
    loader: Option<Loader>,
    progress: Mutex<Progress>,
    changed: Condvar,
    /// Bumped by `cancel`; a stream started under an older value stops.
    generation: AtomicU64,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The Kokoro engine. See the module docs.
#[derive(Clone)]
pub struct Kokoro {
    inner: Arc<Inner>,
}

impl Kokoro {
    pub fn new(config: Config) -> Kokoro {
        let manager = Manager::new(config.model_dir, config.files, config.backoff);
        Kokoro {
            inner: Arc::new(Inner {
                manager,
                runtime: config.runtime,
                download: config.download,
                backoff: config.backoff,
                fallback: config.fallback,
                fallback_ok: OnceLock::new(),
                loader: config.loader,
                progress: Mutex::new(Progress {
                    stage: Stage::Idle,
                    loaded: None,
                    unavailable: None,
                    load_failed: None,
                }),
                changed: Condvar::new(),
                generation: AtomicU64::new(0),
            }),
        }
    }

    /// The model files are in place and verified (no hashing).
    pub fn model_present(&self) -> bool {
        self.inner.manager.present()
    }

    /// The model is loaded: syntheses use Kokoro itself.
    pub fn is_ready(&self) -> bool {
        self.inner.lock().loaded.is_some()
    }

    /// Verifying, downloading or loading the model right now (a host keeps
    /// running meanwhile rather than idling out mid-download).
    pub fn is_preparing(&self) -> bool {
        let p = self.inner.lock();
        p.loaded.is_none() && (p.stage != Stage::Idle || self.inner.manager.is_running())
    }

    /// The fallback, if it can speak here. The first call warms it.
    fn usable_fallback(&self) -> Option<&Arc<dyn Engine>> {
        let f = self.inner.fallback.as_ref()?;
        let ok = *self
            .inner
            .fallback_ok
            .get_or_init(|| f.warm().is_ok() && !f.voices().is_empty());
        ok.then_some(f)
    }

    /// With no fallback that can speak: wait while Kokoro downloads or
    /// loads, up to `timeout`; `Cancelled` if `cancel` came meanwhile.
    fn wait_kokoro(&self, generation: u64, timeout: Duration) -> Result<Option<Arc<Loaded>>> {
        let end = Instant::now() + timeout;
        let mut p = self.inner.lock();
        loop {
            if let Some(l) = &p.loaded {
                return Ok(Some(l.clone()));
            }
            if self.inner.generation.load(Ordering::SeqCst) != generation {
                return Err(Error::Cancelled);
            }
            let preparing = p.stage != Stage::Idle || self.inner.manager.is_running();
            let now = Instant::now();
            if p.unavailable.is_some() || !preparing || now >= end {
                return Ok(None);
            }
            p = self
                .inner
                .changed
                .wait_timeout(p, (end - now).min(Duration::from_millis(50)))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Start getting ready in the background (verify, download, load) if
    /// nothing runs and no failure is waiting for its retry. Cheap to call
    /// often. Also what `sonarad` calls at startup to prefetch the model.
    pub fn prepare(&self) {
        start(&self.inner);
    }

    /// Wait until the engine is ready or `timeout` passed; true if ready.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        let mut p = self.inner.lock();
        loop {
            if p.loaded.is_some() {
                return true;
            }
            let now = Instant::now();
            if now >= end || (p.stage == Stage::Idle && !self.inner.manager.is_running()) {
                return p.loaded.is_some();
            }
            p = self
                .inner
                .changed
                .wait_timeout(p, (end - now).min(Duration::from_millis(50)))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Wait while files already on disk are verified and loaded (a few
    /// seconds at most), so a synthesis does not switch to the fallback
    /// voice for it. A download (a file turned out bad) ends the wait.
    fn wait_loading(&self, generation: u64) {
        let end = Instant::now() + LOAD_WAIT;
        let mut p = self.inner.lock();
        while p.stage == Stage::Loading
            && !matches!(
                self.inner.manager.phase(),
                Phase::Downloading { .. } | Phase::Failed { .. }
            )
            && Instant::now() < end
            && self.inner.generation.load(Ordering::SeqCst) == generation
        {
            p = self
                .inner
                .changed
                .wait_timeout(p, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn voice_id(wanted: &str) -> Result<&'static str> {
        if wanted.is_empty() {
            return Ok(DEFAULT_VOICE);
        }
        let w = wanted.strip_prefix("kokoro:").unwrap_or(wanted);
        ENGLISH_VOICES
            .iter()
            .find(|v| v.eq_ignore_ascii_case(w) || voices::display_name(v).eq_ignore_ascii_case(w))
            .copied()
            .ok_or_else(|| Error::UnknownVoice(wanted.to_string()))
    }

    fn not_ready(&self) -> Error {
        let s = self.status();
        let why = s.message.unwrap_or_else(|| match s.readiness {
            Readiness::Downloading => "its model is downloading".into(),
            _ => "its model is not loaded yet".into(),
        });
        Error::Engine(format!("Kokoro is not ready: {why}"))
    }
}

/// Kick off preparation (see `Kokoro::prepare`).
fn start(inner: &Arc<Inner>) {
    // `present`: the files are on disk, so this is verify-and-load.
    let present = {
        let mut p = inner.lock();
        if p.loaded.is_some() || p.unavailable.is_some() || p.stage != Stage::Idle {
            return;
        }
        if let Some((_, retry_at, _)) = &p.load_failed {
            if Instant::now() < *retry_at {
                return;
            }
        }
        let present = inner.manager.on_disk();
        if !present && !inner.download {
            if p.unavailable.is_none() {
                p.unavailable = Some(format!(
                    "the Kokoro model is not installed in {} and downloads are off",
                    inner.manager.dir().display()
                ));
            }
            return;
        }
        if !inner.manager.begin(Instant::now()) {
            return;
        }
        p.stage = if present {
            Stage::Loading
        } else {
            Stage::Fetching
        };
        present
    };
    let worker = inner.clone();
    let spawned = std::thread::Builder::new()
        .name("sonara-kokoro-prepare".into())
        .spawn(move || prepare(&worker, present));
    if spawned.is_err() {
        let _ = inner.manager.fetch_failed("cannot start a thread".into());
        inner.lock().stage = Stage::Idle;
        inner.changed.notify_all();
    }
}

/// The preparation thread: files, then the model. A panic in either (a
/// library bug) counts as a failure with backoff, never a stuck download.
fn prepare(inner: &Inner, present: bool) {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let fetched = catch_unwind(AssertUnwindSafe(|| inner.manager.fetch())).unwrap_or_else(|_| {
        inner
            .manager
            .fetch_failed("the model download panicked".into())
    });
    if fetched.is_ok() {
        if !present {
            inner.lock().stage = Stage::Loading;
            inner.changed.notify_all();
        }
        let result = catch_unwind(AssertUnwindSafe(|| load(inner)))
            .unwrap_or_else(|_| Err(Failure::Retry("loading the model panicked".into())));
        let mut p = inner.lock();
        match result {
            Ok(loaded) => {
                p.loaded = Some(Arc::new(loaded));
                p.load_failed = None;
            }
            Err(Failure::Unavailable(why)) => p.unavailable = Some(why),
            Err(Failure::Retry(why)) => {
                let n = p.load_failed.as_ref().map_or(0, |(_, _, n)| *n) + 1;
                p.load_failed = Some((why, Instant::now() + inner.backoff.delay(n), n));
            }
        }
    }
    inner.lock().stage = Stage::Idle;
    inner.changed.notify_all();
}

enum Failure {
    Unavailable(String),
    Retry(String),
}

fn load(inner: &Inner) -> std::result::Result<Loaded, Failure> {
    let model_path = inner.manager.path(download::MODEL_FILE);
    let model = match &inner.loader {
        Some(loader) => loader(&model_path).map_err(|e| Failure::Retry(e.to_string()))?,
        None => {
            model::init_runtime(&inner.runtime).map_err(Failure::Unavailable)?;
            Box::new(model::OrtModel::load(&model_path).map_err(|e| Failure::Retry(e.to_string()))?)
                as Box<dyn Acoustic>
        }
    };
    let bytes = std::fs::read(inner.manager.path(download::VOICES_FILE))
        .map_err(|e| Failure::Retry(format!("cannot read the Kokoro voices: {e}")))?;
    let styles =
        Styles::parse(&bytes, ENGLISH_VOICES).map_err(|e| Failure::Retry(e.to_string()))?;
    Ok(Loaded {
        model: Mutex::new(model),
        styles,
        phonemizer: phonemizer(),
    })
}

impl Engine for Kokoro {
    fn id(&self) -> EngineId {
        ID
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Permissive
    }

    fn voices(&self) -> Vec<Voice> {
        let installed = self.is_ready() || self.model_present();
        ENGLISH_VOICES
            .iter()
            .map(|id| Voice {
                id: id.to_string(),
                name: voices::display_name(id),
                language: voices::language(id).to_string(),
                engine: ID,
                license_class: LicenseClass::Permissive,
                installed,
            })
            .collect()
    }

    /// Start preparing; when the model is on disk, wait for it to load so
    /// the first sentence is Kokoro's. Warms the fallback too. Fails only
    /// when Kokoro cannot run here and there is no fallback.
    fn warm(&self) -> Result<()> {
        self.prepare();
        self.wait_loading(self.inner.generation.load(Ordering::SeqCst));
        let fallback = if self.is_ready() {
            None
        } else {
            self.usable_fallback()
        };
        let p = self.inner.lock();
        match (&p.unavailable, fallback) {
            (Some(why), None) => Err(Error::Engine(format!("Kokoro cannot run: {why}"))),
            _ => Ok(()),
        }
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        let voice = Self::voice_id(voice)?;
        let generation = self.inner.generation.load(Ordering::SeqCst);
        self.prepare();
        self.wait_loading(generation);
        let loaded = self.inner.lock().loaded.clone();
        let loaded = match loaded {
            Some(l) => l,
            // The fallback's own default voice: Kokoro's ids mean nothing
            // to it.
            None => match self.usable_fallback() {
                Some(f) => return f.synthesize(text, "", rate),
                None => match self.wait_kokoro(generation, NO_FALLBACK_WAIT)? {
                    Some(l) => l,
                    None => return Err(self.not_ready()),
                },
            },
        };
        let ps = loaded.phonemizer.phonemize(text);
        Ok(Box::new(Stream {
            inner: self.inner.clone(),
            loaded,
            batches: phonemes::batches(&ps).into(),
            voice,
            speed: speed(rate),
            generation,
            done: false,
        }))
    }

    fn cancel(&self) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        self.inner.changed.notify_all();
        if let Some(f) = &self.inner.fallback {
            f.cancel();
        }
    }

    fn status(&self) -> EngineStatus {
        let p = self.inner.lock();
        if p.loaded.is_some() {
            return EngineStatus::ready();
        }
        // A fallback known not to speak here is not named.
        let fallback = match self.inner.fallback_ok.get() {
            Some(false) => None,
            _ => self.inner.fallback.as_ref().map(|f| f.id()),
        };
        let status = |readiness, progress, message| EngineStatus {
            readiness,
            progress,
            fallback,
            message,
            reason: None,
        };
        if let Some(why) = &p.unavailable {
            return status(Readiness::Unavailable, None, Some(why.clone()));
        }
        let phase = self.inner.manager.phase();
        if let Phase::Downloading { done, total } = phase {
            return status(Readiness::Downloading, Some((done, total)), None);
        }
        if p.stage == Stage::Loading {
            return status(Readiness::Loading, None, None);
        }
        match phase {
            Phase::Failed { reason, .. } => status(Readiness::Waiting, None, Some(reason)),
            _ => match &p.load_failed {
                Some((why, _, _)) => status(Readiness::Waiting, None, Some(why.clone())),
                None => status(Readiness::Loading, None, None),
            },
        }
    }
}

/// One synthesis: a model call per batch, as the reader pulls.
struct Stream {
    inner: Arc<Inner>,
    loaded: Arc<Loaded>,
    batches: VecDeque<String>,
    voice: &'static str,
    speed: f32,
    generation: u64,
    done: bool,
}

impl Iterator for Stream {
    type Item = Result<PcmChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        if self.inner.generation.load(Ordering::SeqCst) != self.generation {
            self.done = true;
            return Some(Err(Error::Cancelled));
        }
        let batch = self.batches.pop_front()?;
        let tokens = phonemes::tokens(&batch);
        let Some(style) = self.loaded.styles.style(self.voice, tokens.len()) else {
            self.done = true;
            return Some(Err(Error::UnknownVoice(self.voice.to_string())));
        };
        let audio = {
            let mut model = self.loaded.model.lock().unwrap_or_else(|e| e.into_inner());
            model.infer(&tokens, style, self.speed)
        };
        if self.inner.generation.load(Ordering::SeqCst) != self.generation {
            self.done = true;
            return Some(Err(Error::Cancelled));
        }
        Some(audio.map(|a| PcmChunk {
            samples: to_i16(&normalize_rms(trim(&a))),
            sample_rate: SAMPLE_RATE,
            channels: 1,
        }))
    }
}

/// Trim leading and trailing silence (60 dB below the loudest 2048-sample
/// frame, hop 512), as kokoro-onnx.
pub fn trim(a: &[f32]) -> &[f32] {
    let (fl, hop) = (2048usize, 512usize);
    if a.len() < fl {
        return a;
    }
    let frames: Vec<f32> = (0..=(a.len() - fl) / hop)
        .map(|i| {
            let f = &a[i * hop..i * hop + fl];
            (f.iter().map(|x| x * x).sum::<f32>() / fl as f32).sqrt()
        })
        .collect();
    let peak = frames.iter().cloned().fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return a;
    }
    let thr = peak * 10f32.powf(-60.0 / 20.0);
    let first = frames.iter().position(|&r| r > thr).unwrap_or(0);
    let last = frames
        .iter()
        .rposition(|&r| r > thr)
        .unwrap_or(frames.len() - 1);
    let s = first * hop;
    let e = ((last + 1) * hop + fl).min(a.len());
    &a[s..e]
}

/// The Python reader's `normalize_rms` (#81): voiced RMS to 0.08, peak at
/// most 0.97, so Kokoro and OneCore play at the same loudness.
pub fn normalize_rms(x: &[f32]) -> Vec<f32> {
    let frame = 480;
    if x.len() < frame {
        return x.to_vec();
    }
    let rms: Vec<f32> = x
        .chunks_exact(frame)
        .map(|f| (f.iter().map(|v| v * v).sum::<f32>() / frame as f32).sqrt())
        .collect();
    let gate = (rms.iter().cloned().fold(0.0f32, f32::max) * 0.1).max(1e-4);
    let voiced: Vec<f32> = rms.into_iter().filter(|r| *r > gate).collect();
    if voiced.is_empty() {
        return x.to_vec();
    }
    let level = voiced.iter().sum::<f32>() / voiced.len() as f32;
    let mut g = 0.08 / level;
    let pk = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if pk * g > 0.97 {
        g = 0.97 / pk;
    }
    x.iter().map(|v| v * g).collect()
}

fn to_i16(x: &[f32]) -> Vec<i16> {
    x.iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_maps_to_kokoro_speed() {
        let table = [
            (0, 0.5),
            (100, 0.5),
            (150, 0.75),
            (200, 1.0),
            (250, 1.25),
            (300, 1.5),
            (400, 2.0),
            (1000, 2.0),
            (u32::MAX, 2.0),
        ];
        for (wpm, expected) in table {
            assert_eq!(speed(wpm), expected, "wpm {wpm}");
        }
    }

    #[test]
    fn voices_are_matched_by_id_name_or_prefix() {
        assert_eq!(Kokoro::voice_id("").unwrap(), "af_heart");
        assert_eq!(Kokoro::voice_id("AF_Sarah").unwrap(), "af_sarah");
        assert_eq!(Kokoro::voice_id("kokoro:bm_george").unwrap(), "bm_george");
        assert_eq!(Kokoro::voice_id("Heart (Kokoro)").unwrap(), "af_heart");
        assert!(matches!(
            Kokoro::voice_id("zf_xiaobei"),
            Err(Error::UnknownVoice(_))
        ));
    }

    #[test]
    fn silence_is_trimmed_and_loudness_normalized() {
        let mut a = vec![0.0f32; 4096];
        a.extend((0..4800).map(|i| if i % 2 == 0 { 0.5 } else { -0.5 }));
        a.extend(vec![0.0f32; 4096]);
        let t = trim(&a);
        assert!(t.len() < a.len() && t.len() >= 4800, "{}", t.len());
        let n = normalize_rms(t);
        let peak = n.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        // Voiced RMS 0.08 (the tone's peak equals its RMS; the trimmed
        // edges lower the average a little).
        assert!((0.08..0.09).contains(&peak), "{peak}");
        assert_eq!(to_i16(&[2.0, -2.0, 0.0]), vec![32767, -32767, 0]);
    }
}
