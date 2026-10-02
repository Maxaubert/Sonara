//! The Kokoro engine with a fake acoustic model (no ONNX Runtime, no real
//! model): fallback while not ready, sentence batches, rate, voices,
//! cancel, and an install without ONNX Runtime.
#![cfg(feature = "kokoro")]
mod common;

use common::{sha256, voices_npz, Fault, FileServer, TempDir};
use sonara_engine::fake::{self, FakeEngine};
use sonara_engine::kokoro::download::{Backoff, ModelFile, MODEL_FILE, VOICES_FILE};
use sonara_engine::kokoro::model::Acoustic;
use sonara_engine::kokoro::voices::{ENGLISH_VOICES, STYLE_DIM, STYLE_ROWS};
use sonara_engine::kokoro::{self, Config, Kokoro, Loader};
use sonara_engine::{Engine, Error, Readiness, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the fake model was asked: token count, style value, speed.
type Calls = Arc<Mutex<Vec<(usize, f32, f32)>>>;

struct FakeModel {
    calls: Calls,
}

impl Acoustic for FakeModel {
    fn infer(&mut self, tokens: &[i64], style: &[f32], speed: f32) -> Result<Vec<f32>> {
        assert_eq!(style.len(), STYLE_DIM);
        self.calls
            .lock()
            .unwrap()
            .push((tokens.len(), style[0], speed));
        // 10 ms of tone per token: loud enough to survive the trim.
        Ok((0..tokens.len() * 240)
            .map(|i| if i % 20 < 10 { 0.3 } else { -0.3 })
            .collect())
    }
}

fn loader(calls: &Calls) -> Loader {
    let calls = calls.clone();
    Arc::new(move |path| {
        assert!(path.ends_with(MODEL_FILE));
        Ok(Box::new(FakeModel {
            calls: calls.clone(),
        }) as Box<dyn Acoustic>)
    })
}

struct Fixture {
    dir: TempDir,
    model: Vec<u8>,
    voices: Vec<u8>,
    calls: Calls,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture {
            dir: TempDir::new("kokoro"),
            model: b"not really an onnx model".to_vec(),
            voices: voices_npz(ENGLISH_VOICES, STYLE_ROWS, STYLE_DIM),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn model_dir(&self) -> PathBuf {
        self.dir.path().join("models/kokoro/v1.0")
    }

    fn files(&self, base: &str) -> Vec<ModelFile> {
        [(MODEL_FILE, &self.model), (VOICES_FILE, &self.voices)]
            .into_iter()
            .map(|(name, data)| ModelFile {
                name: name.into(),
                url: format!("{base}/{name}"),
                sha256: sha256(data),
                size: data.len() as u64,
            })
            .collect()
    }

    fn seed(&self) {
        std::fs::create_dir_all(self.model_dir()).unwrap();
        std::fs::write(self.model_dir().join(MODEL_FILE), &self.model).unwrap();
        std::fs::write(self.model_dir().join(VOICES_FILE), &self.voices).unwrap();
    }

    fn server(&self) -> FileServer {
        FileServer::start(&[
            (MODEL_FILE, self.model.clone()),
            (VOICES_FILE, self.voices.clone()),
        ])
    }

    fn config(&self, base: &str) -> Config {
        let mut c = Config::new(self.model_dir(), self.dir.path().join("no-onnxruntime.dll"));
        c.files = self.files(base);
        c.backoff = Backoff {
            first: Duration::from_millis(300),
            max: Duration::from_secs(5),
        };
        c.fallback = Some(Arc::new(FakeEngine::new()));
        c.loader = Some(loader(&self.calls));
        c
    }
}

fn chunks(e: &Kokoro, text: &str, voice: &str, rate: u32) -> Vec<sonara_engine::PcmChunk> {
    e.synthesize(text, voice, rate)
        .unwrap()
        .collect::<Result<Vec<_>>>()
        .unwrap()
}

#[test]
fn a_present_model_speaks_one_batch_per_sentence() {
    let fx = Fixture::new();
    fx.seed();
    let e = Kokoro::new(fx.config("http://127.0.0.1:9/unused"));
    assert_eq!(e.id(), kokoro::ID);
    e.warm().unwrap();
    assert!(e.is_ready());
    assert_eq!(e.status().readiness, Readiness::Ready);
    let out = chunks(&e, "Hello there. How are you? Fine!", "", 200);
    assert_eq!(out.len(), 3);
    for c in &out {
        assert_eq!((c.sample_rate, c.channels), (kokoro::SAMPLE_RATE, 1));
        assert!(!c.samples.is_empty());
    }
    let calls = fx.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 3);
    // Default voice af_heart is voice 0 of the catalogue; speed 1.0.
    assert!(calls
        .iter()
        .all(|(n, style, speed)| *n > 0 && *style == 0.0 && *speed == 1.0));
}

#[test]
fn rate_and_voice_reach_the_model() {
    let fx = Fixture::new();
    fx.seed();
    let e = Kokoro::new(fx.config("http://127.0.0.1:9/unused"));
    e.warm().unwrap();
    chunks(&e, "Quick.", "af_sarah", 250);
    chunks(&e, "Slow.", "bm_george", 100);
    let calls = fx.calls.lock().unwrap().clone();
    let sarah = ENGLISH_VOICES
        .iter()
        .position(|v| *v == "af_sarah")
        .unwrap() as f32;
    let george = ENGLISH_VOICES
        .iter()
        .position(|v| *v == "bm_george")
        .unwrap() as f32;
    assert_eq!((calls[0].1, calls[0].2), (sarah, 1.25));
    assert_eq!((calls[1].1, calls[1].2), (george, 0.5));
    assert!(matches!(
        e.synthesize("x", "zf_xiaobei", 200),
        Err(Error::UnknownVoice(_))
    ));
}

#[test]
fn model_download_falls_back_to_the_fallback_engine_until_ready() {
    let fx = Fixture::new();
    let server = fx.server();
    let e = Kokoro::new(fx.config(&server.base));
    assert!(e.voices().iter().all(|v| !v.installed));
    // Not on disk: warm starts the download and returns at once.
    e.warm().unwrap();
    let out = chunks(&e, "Hello.", "af_heart", 200);
    assert!(out
        .iter()
        .all(|c| c.sample_rate == fake::SAMPLE_RATE || c.sample_rate == kokoro::SAMPLE_RATE));
    assert!(e.wait_ready(Duration::from_secs(20)), "{:?}", e.status());
    assert!(e.voices().iter().all(|v| v.installed));
    let out = chunks(&e, "Hello.", "af_heart", 200);
    assert!(out.iter().all(|c| c.sample_rate == kokoro::SAMPLE_RATE));
    assert_eq!(server.requests(), 2);
}

#[test]
fn model_download_offline_speaks_with_the_fallback_and_retries_later() {
    let fx = Fixture::new();
    let server = fx.server();
    server.fault(MODEL_FILE, Fault::Status(503));
    let e = Kokoro::new(fx.config(&server.base));
    e.warm().unwrap();
    // Wait for the failed attempt to settle.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while e.status().readiness != Readiness::Waiting {
        assert!(std::time::Instant::now() < deadline, "{:?}", e.status());
        std::thread::sleep(Duration::from_millis(10));
    }
    let s = e.status();
    assert_eq!(s.fallback, Some(fake::ID));
    assert!(s.message.unwrap().contains("503"));
    // Every sentence meanwhile is the fallback's, and none starts a fetch.
    for _ in 0..20 {
        let out = chunks(&e, "Still here.", "", 200);
        assert!(out.iter().all(|c| c.sample_rate == fake::SAMPLE_RATE));
    }
    assert_eq!(server.requests(), 1);
    // After the backoff the next sentence starts a new attempt, which works.
    std::thread::sleep(Duration::from_millis(350));
    chunks(&e, "Again.", "", 200);
    assert!(e.wait_ready(Duration::from_secs(20)), "{:?}", e.status());
}

#[test]
fn without_onnx_runtime_kokoro_is_unavailable_and_the_fallback_speaks() {
    let fx = Fixture::new();
    fx.seed();
    let mut config = fx.config("http://127.0.0.1:9/unused");
    config.loader = None; // the real loader, with no onnxruntime.dll
    let e = Kokoro::new(config);
    e.warm().unwrap();
    assert!(!e.wait_ready(Duration::from_secs(5)));
    let s = e.status();
    assert_eq!(s.readiness, Readiness::Unavailable);
    assert!(
        s.message.unwrap().contains("ONNX Runtime"),
        "{:?}",
        e.status()
    );
    let out = chunks(&e, "Hello.", "", 200);
    assert!(out.iter().all(|c| c.sample_rate == fake::SAMPLE_RATE));
}

#[test]
fn without_a_fallback_a_not_ready_engine_errors() {
    let fx = Fixture::new();
    let mut config = fx.config("http://127.0.0.1:9/unused");
    config.fallback = None;
    config.download = false;
    let e = Kokoro::new(config);
    assert!(e.warm().is_err());
    assert_eq!(e.status().readiness, Readiness::Unavailable);
    let err = e.synthesize("Hello.", "", 200).err().unwrap();
    assert!(err.to_string().contains("downloads are off"), "{err}");
}

#[test]
fn cancel_ends_a_stream_in_flight() {
    let fx = Fixture::new();
    fx.seed();
    let e = Kokoro::new(fx.config("http://127.0.0.1:9/unused"));
    e.warm().unwrap();
    let mut s = e.synthesize("One. Two. Three.", "", 200).unwrap();
    assert!(s.next().unwrap().is_ok());
    e.cancel();
    assert_eq!(s.next().unwrap(), Err(Error::Cancelled));
    assert!(s.next().is_none());
    // A new synthesis after the cancel works.
    assert_eq!(chunks(&e, "Four.", "", 200).len(), 1);
}

#[test]
fn voices_list_the_english_catalogue() {
    let fx = Fixture::new();
    let e = Kokoro::new(fx.config("http://127.0.0.1:9/unused"));
    let v = e.voices();
    assert_eq!(v.len(), 28);
    assert_eq!(v[0].id, "af_heart");
    assert_eq!(v[0].name, "Heart (Kokoro)");
    assert!(v
        .iter()
        .any(|v| v.id == "bm_george" && v.language == "en-GB"));
    assert!(v
        .iter()
        .all(|v| v.engine == kokoro::ID
            && v.license_class == sonara_engine::LicenseClass::Permissive));
}
