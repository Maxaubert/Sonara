//! The WinRT side of the OneCore engine.
use super::{choose_voice, classify_failure, speaking_rate, VoiceInfo, ID};
use crate::{wav, Engine, EngineId, Error, LicenseClass, PcmStream, Result, Voice};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use windows::core::{AgileReference, Interface, HSTRING};
use windows::Media::SpeechSynthesis::{SpeechSynthesizer, VoiceInformation};
use windows::Storage::Streams::DataReader;
use windows_future::{IAsyncInfo, IAsyncOperation};

/// Short text `warm` synthesizes to prove the voice data is there.
const PROBE: &str = "Sonara.";

/// Windows OneCore speech. A fresh `SpeechSynthesizer` per call keeps
/// concurrent syntheses (the reader prefetches one chunk ahead) from
/// sharing voice or rate settings.
#[derive(Default)]
pub struct OneCore {
    /// Bumped by `cancel`; a synthesis that sees a newer epoch is abandoned.
    epoch: AtomicU64,
    next_op: AtomicU64,
    /// WinRT operations in flight (the synthesis and the stream load), as
    /// agile references so `cancel` may run on any thread.
    in_flight: Mutex<HashMap<u64, AgileReference<IAsyncInfo>>>,
    /// Set once a synthesis found the voice data missing (D7), so `voices`
    /// reports them as not installed.
    data_missing: AtomicBool,
}

fn platform(e: windows::core::Error) -> Error {
    classify_failure(e.code().0, &e.message(), &[])
}

impl OneCore {
    pub fn new() -> Self {
        Self::default()
    }

    fn voice_infos(&self) -> Result<Vec<(VoiceInfo, VoiceInformation)>> {
        let all = SpeechSynthesizer::AllVoices().map_err(platform)?;
        let mut out = Vec::new();
        for v in all {
            let info = VoiceInfo {
                id: v.Id().map_err(platform)?.to_string(),
                name: v.DisplayName().map_err(platform)?.to_string(),
                language: v.Language().map_err(platform)?.to_string(),
            };
            out.push((info, v));
        }
        Ok(out)
    }

    fn track(&self, op: &IAsyncOperation<impl windows::core::RuntimeType>) -> Option<u64> {
        let info: IAsyncInfo = op.cast().ok()?;
        let info = AgileReference::new(&info).ok()?;
        let key = self.next_op.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut map) = self.in_flight.lock() {
            map.insert(key, info);
        }
        Some(key)
    }

    fn untrack(&self, key: Option<u64>) {
        if let (Some(key), Ok(mut map)) = (key, self.in_flight.lock()) {
            map.remove(&key);
        }
    }

    fn check_cancel(&self, epoch: u64) -> Result<()> {
        if self.epoch.load(Ordering::SeqCst) != epoch {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Synthesize to the WAV bytes WinRT returns.
    fn synthesize_wav(&self, text: &str, voice: &str, rate: u32) -> Result<Vec<u8>> {
        let epoch = self.epoch.load(Ordering::SeqCst);
        let voices = self.voice_infos()?;
        let names: Vec<String> = voices.iter().map(|(i, _)| i.name.clone()).collect();
        let fail = |e: windows::core::Error| {
            let err = classify_failure(e.code().0, &e.message(), &names);
            if matches!(err, Error::MissingVoiceData { .. }) {
                self.data_missing.store(true, Ordering::SeqCst);
            }
            err
        };
        let plain: Vec<VoiceInfo> = voices.iter().map(|(i, _)| i.clone()).collect();
        let chosen = choose_voice(&plain, voice)?;
        let synth = SpeechSynthesizer::new().map_err(fail)?;
        if let Some(c) = chosen {
            if let Some((_, winrt_voice)) = voices.iter().find(|(i, _)| i == c) {
                synth.SetVoice(winrt_voice).map_err(fail)?;
            }
        }
        synth
            .Options()
            .and_then(|o| o.SetSpeakingRate(speaking_rate(rate)))
            .map_err(fail)?;

        let op = synth
            .SynthesizeTextToStreamAsync(&HSTRING::from(text))
            .map_err(fail)?;
        let key = self.track(&op);
        let stream = op.join();
        self.untrack(key);
        self.check_cancel(epoch)?;
        let stream = stream.map_err(fail)?;

        let size = stream.Size().map_err(fail)?;
        let size = u32::try_from(size)
            .map_err(|_| Error::Engine(format!("OneCore returned {size} bytes, too many")))?;
        let reader = stream
            .GetInputStreamAt(0)
            .and_then(|input| DataReader::CreateDataReader(&input))
            .map_err(fail)?;
        let load: IAsyncOperation<u32> = reader
            .LoadAsync(size)
            .and_then(|l| l.cast())
            .map_err(fail)?;
        let key = self.track(&load);
        let loaded = load.join();
        self.untrack(key);
        self.check_cancel(epoch)?;
        let loaded = loaded.map_err(fail)?;
        let mut buf = vec![0u8; loaded as usize];
        reader.ReadBytes(&mut buf).map_err(fail)?;
        self.check_cancel(epoch)?;
        self.data_missing.store(false, Ordering::SeqCst);
        Ok(buf)
    }
}

impl Engine for OneCore {
    fn id(&self) -> EngineId {
        ID
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Os
    }

    /// The listed voices; an empty list when WinRT cannot list them.
    fn voices(&self) -> Vec<Voice> {
        let installed = !self.data_missing.load(Ordering::SeqCst);
        self.voice_infos()
            .unwrap_or_default()
            .into_iter()
            .map(|(i, _)| Voice {
                id: i.id,
                name: i.name,
                language: i.language,
                engine: ID,
                license_class: LicenseClass::Os,
                installed,
            })
            .collect()
    }

    /// Synthesize a short probe: listing voices is not enough, a listed voice
    /// can have its data missing (D7).
    fn warm(&self) -> Result<()> {
        if self.voice_infos()?.is_empty() {
            return Err(Error::NoVoices);
        }
        self.synthesize_wav(PROBE, "", 200).map(|_| ())
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        let bytes = self.synthesize_wav(text, voice, rate)?;
        let pcm = wav::decode(&bytes)?;
        Ok(Box::new(std::iter::once(Ok(pcm))))
    }

    fn cancel(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        if let Ok(map) = self.in_flight.lock() {
            for info in map.values() {
                if let Ok(info) = info.resolve() {
                    let _ = info.Cancel();
                }
            }
        }
    }
}
