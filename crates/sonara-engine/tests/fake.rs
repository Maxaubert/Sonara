//! The fake engine is deterministic: tests of higher crates rely on it.
use sonara_engine::fake::{self, FakeEngine};
use sonara_engine::{Engine, Error, PcmChunk};

fn collect(engine: &FakeEngine, text: &str, voice: &str, rate: u32) -> Vec<PcmChunk> {
    engine
        .synthesize(text, voice, rate)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn same_input_gives_the_same_audio() {
    let e = FakeEngine::new();
    let a = collect(&e, "Hello there, world.", "", 200);
    let b = collect(&e, "Hello there, world.", "tone", 200);
    assert_eq!(a, b);
    assert_eq!(e.syntheses(), 2);
}

#[test]
fn length_follows_text_and_rate() {
    let e = FakeEngine::new();
    let total = |text: &str, rate| -> usize {
        collect(&e, text, "", rate)
            .iter()
            .map(|c| c.samples.len())
            .sum()
    };
    // 19 chars * 10 ms = 190 ms at 16 kHz.
    assert_eq!(total("Hello there, world.", 200), 3040);
    assert_eq!(total("Hello there, world.", 400), 1520);
    assert_eq!(total("Hello there, world.", 100), 6080);
    // Never empty, whatever the rate.
    assert_eq!(total("a", u32::MAX), 1);
}

#[test]
fn audio_comes_in_bounded_mono_chunks() {
    let e = FakeEngine::new();
    let chunks = collect(&e, &"x".repeat(25), "", 200); // 4000 samples
    assert_eq!(
        chunks.iter().map(|c| c.samples.len()).collect::<Vec<_>>(),
        vec![1600, 1600, 800]
    );
    assert!(chunks
        .iter()
        .all(|c| c.sample_rate == fake::SAMPLE_RATE && c.channels == 1));
    assert_eq!(chunks[0].duration_ms(), 100);
}

#[test]
fn tone_is_a_square_wave_and_silence_is_zero() {
    let tone = FakeEngine::render("abcd", "tone", 200).unwrap();
    assert_eq!(&tone[..3], &[8000, 8000, 8000]);
    assert_eq!(tone[20], -8000); // half period of 400 Hz at 16 kHz = 20 samples
    assert_eq!(tone[40], 8000);
    let silence = FakeEngine::render("abcd", "silence", 200).unwrap();
    assert!(silence.iter().all(|&s| s == 0));
    assert_eq!(silence.len(), tone.len());
}

#[test]
fn fail_mark_and_unknown_voice_are_errors() {
    let e = FakeEngine::new();
    assert!(matches!(
        e.synthesize("Bad [fail] chunk.", "", 200).err(),
        Some(Error::Engine(_))
    ));
    assert_eq!(
        e.synthesize("Hi.", "nobody", 200).err(),
        Some(Error::UnknownVoice("nobody".into()))
    );
}

#[test]
fn cancel_is_counted() {
    let e = FakeEngine::new();
    e.cancel();
    e.cancel();
    assert_eq!(e.cancels(), 2);
    assert!(e.warm().is_ok());
}
