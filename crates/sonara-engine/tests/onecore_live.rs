//! Live OneCore checks against this machine's real voices. Ignored by
//! default (CI has no voices, and a PC with missing voice data fails them);
//! run on request:
//! `cargo test -p sonara-engine --test onecore_live -- --ignored --nocapture`
#![cfg(all(windows, feature = "onecore"))]
use sonara_engine::onecore::OneCore;
use sonara_engine::{Engine, Error, LicenseClass};

#[test]
#[ignore = "live: needs Windows OneCore voices with their data installed"]
fn onecore_speaks_a_sentence() {
    let engine = OneCore::new();
    assert_eq!(engine.license_class(), LicenseClass::Os);
    let voices = engine.voices();
    println!(
        "voices: {:?}",
        voices.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    match engine.warm() {
        Ok(()) => {}
        Err(e @ (Error::MissingVoiceData { .. } | Error::NoVoices)) => {
            panic!("OneCore cannot speak on this PC: {e}")
        }
        Err(e) => panic!("warm failed: {e}"),
    }
    let chunks: Vec<_> = engine
        .synthesize("Sonara live check.", "", 200)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let samples: usize = chunks.iter().map(|c| c.samples.len()).sum();
    assert!(samples > 0);
    println!(
        "{} samples at {} Hz, {} channel(s)",
        samples, chunks[0].sample_rate, chunks[0].channels
    );
}
