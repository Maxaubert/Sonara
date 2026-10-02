//! A deterministic engine for tests (feature `test-util`): the same text,
//! voice and rate always give the same samples, with no OS or model.
//!
//! - Length: `MS_PER_CHAR` per character of text at rate 200, scaled by
//!   200 / rate (a faster rate gives shorter audio), at least one sample.
//! - Voice `tone` (the default, also `""`) is a square wave of `TONE_HZ`;
//!   voice `silence` is all zeros. Any other voice is `Error::UnknownVoice`.
//! - Text containing `FAIL_MARK` fails with `Error::Engine`, so tests can
//!   make one chunk of an item fail.
//! - The audio comes as chunks of at most `CHUNK_SAMPLES` samples.
use crate::{Engine, EngineId, Error, LicenseClass, PcmChunk, PcmStream, Result, Voice};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const ID: EngineId = EngineId("fake");
pub const SAMPLE_RATE: u32 = 16_000;
pub const MS_PER_CHAR: u64 = 10;
pub const TONE_HZ: u32 = 400;
pub const AMPLITUDE: i16 = 8_000;
pub const CHUNK_SAMPLES: usize = 1_600;
pub const FAIL_MARK: &str = "[fail]";

#[derive(Debug, Default)]
pub struct FakeEngine {
    syntheses: AtomicUsize,
    cancels: AtomicUsize,
}

impl FakeEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many times `synthesize` was called.
    pub fn syntheses(&self) -> usize {
        self.syntheses.load(Ordering::SeqCst)
    }

    /// How many times `cancel` was called.
    pub fn cancels(&self) -> usize {
        self.cancels.load(Ordering::SeqCst)
    }

    /// The whole synthesis as one sample vector.
    pub fn render(text: &str, voice: &str, rate: u32) -> Result<Vec<i16>> {
        if text.contains(FAIL_MARK) {
            return Err(Error::Engine(format!("fake engine failure on '{text}'")));
        }
        let tone = match voice {
            "" | "tone" => true,
            "silence" => false,
            other => return Err(Error::UnknownVoice(other.to_string())),
        };
        let chars = text.chars().count() as u64;
        let ms = chars * MS_PER_CHAR * 200 / rate.max(1) as u64;
        let len = (ms * SAMPLE_RATE as u64 / 1000).max(1) as usize;
        let half_period = (SAMPLE_RATE / TONE_HZ / 2) as usize;
        Ok((0..len)
            .map(|i| match (tone, (i / half_period) % 2) {
                (false, _) => 0,
                (true, 0) => AMPLITUDE,
                (true, _) => -AMPLITUDE,
            })
            .collect())
    }
}

impl Engine for FakeEngine {
    fn id(&self) -> EngineId {
        ID
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Permissive
    }

    fn voices(&self) -> Vec<Voice> {
        ["tone", "silence"]
            .iter()
            .map(|v| Voice {
                id: v.to_string(),
                name: format!("Fake {v}"),
                language: "en-US".into(),
                engine: ID,
                license_class: LicenseClass::Permissive,
                installed: true,
            })
            .collect()
    }

    fn warm(&self) -> Result<()> {
        Ok(())
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        self.syntheses.fetch_add(1, Ordering::SeqCst);
        let samples = Self::render(text, voice, rate)?;
        let chunks: Vec<Result<PcmChunk>> = samples
            .chunks(CHUNK_SAMPLES)
            .map(|c| {
                Ok(PcmChunk {
                    samples: c.to_vec(),
                    sample_rate: SAMPLE_RATE,
                    channels: 1,
                })
            })
            .collect();
        Ok(Box::new(chunks.into_iter()))
    }

    fn cancel(&self) {
        self.cancels.fetch_add(1, Ordering::SeqCst);
    }
}
