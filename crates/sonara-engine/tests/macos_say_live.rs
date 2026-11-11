//! Live checks of the macOS `say` engine against this Mac's real voices.
//! Ignored by default (CI may lack voices); run on request:
//! `cargo test -p sonara-engine --test macos_say_live -- --ignored --nocapture`
//!
//! Nothing is played: `say` writes WAV files, which the engine reads.
//! `SONARA_SAY_WAV=<folder>` keeps a copy of each for listening.
#![cfg(all(target_os = "macos", feature = "macos-say"))]
use sonara_engine::macos_say::{self, MacSay};
use sonara_engine::{wav, Engine, Error, LicenseClass, PcmChunk};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SENTENCE: &str = "The quick brown fox jumps over the lazy dog while the band plays a \
slow song about rivers, mountains and the long road home to the small town by the sea where \
we grew up together.";

fn speak(e: &MacSay, text: &str, rate: u32) -> PcmChunk {
    let chunks: Vec<PcmChunk> = e
        .synthesize(text, "", rate)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(chunks.len(), 1);
    chunks.into_iter().next().unwrap()
}

fn seconds(c: &PcmChunk) -> f64 {
    c.samples.len() as f64 / (c.sample_rate as f64 * c.channels as f64)
}

#[test]
#[ignore = "live: needs this Mac's say voices"]
fn macos_say_speaks_a_sentence_to_pcm() {
    let e = MacSay::new();
    assert_eq!(e.license_class(), LicenseClass::Os);
    let voices = e.voices();
    println!("{} voices", voices.len());
    assert!(!voices.is_empty(), "say lists no voice");
    let infos = macos_say::parse_voices(&String::from_utf8_lossy(
        &std::process::Command::new(macos_say::SAY)
            .args(["-v", "?"])
            .output()
            .unwrap()
            .stdout,
    ));
    println!(
        "default voice: {:?}",
        macos_say::choose_voice(&infos, "")
            .unwrap()
            .map(|v| &v.name)
    );
    let t = Instant::now();
    e.warm().unwrap();
    println!("warm: {:.3} s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let pcm = speak(&e, "Sonara live check.", 200);
    println!(
        "one short sentence: {:.3} s, {} samples at {} Hz",
        t.elapsed().as_secs_f64(),
        pcm.samples.len(),
        pcm.sample_rate
    );
    assert_eq!(pcm.sample_rate, 22_050);
    assert_eq!(pcm.channels, 1);
    assert!(pcm.samples.iter().any(|s| *s != 0));
    if let Ok(d) = std::env::var("SONARA_SAY_WAV") {
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(std::path::Path::new(&d).join("say.wav"), wav::encode(&pcm)).unwrap();
    }
}

/// The rate calibration: Sonara's 200 sounds as fast as `say` on its own,
/// and twice the rate takes about half the time.
#[test]
#[ignore = "live: needs this Mac's say voices"]
fn macos_say_rate_matches_sonara_words_per_minute() {
    let e = MacSay::new();
    let normal = seconds(&speak(&e, SENTENCE, 200));
    let fast = seconds(&speak(&e, SENTENCE, 400));
    let dir = std::env::temp_dir().join(format!("sonara-say-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let plain = dir.join("plain.wav");
    let status = std::process::Command::new(macos_say::SAY)
        .arg("-o")
        .arg(&plain)
        .arg(format!("--data-format={}", macos_say::DATA_FORMAT))
        .arg(SENTENCE)
        .status()
        .unwrap();
    assert!(status.success());
    let own = seconds(&wav::decode(&std::fs::read(&plain).unwrap()).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    println!("say alone {own:.2} s, Sonara 200: {normal:.2} s, Sonara 400: {fast:.2} s");
    let ratio = normal / own;
    assert!(
        (0.85..=1.15).contains(&ratio),
        "200 vs say's own speed: {ratio:.2}"
    );
    let ratio = fast / normal;
    assert!((0.4..=0.65).contains(&ratio), "400 vs 200: {ratio:.2}");
}

#[test]
#[ignore = "live: needs this Mac's say voices"]
fn macos_say_cancel_ends_a_long_synthesis_early() {
    let e = Arc::new(MacSay::new());
    e.warm().unwrap();
    let long = SENTENCE.repeat(40);
    let worker = {
        let e = e.clone();
        std::thread::spawn(move || e.synthesize(&long, "", 200).map(|_| ()))
    };
    std::thread::sleep(Duration::from_millis(150));
    let t = Instant::now();
    e.cancel();
    assert_eq!(worker.join().unwrap().err(), Some(Error::Cancelled));
    println!("cancelled in {:.3} s", t.elapsed().as_secs_f64());
    assert!(t.elapsed() < Duration::from_secs(1));
}
