//! Live Kokoro check with the real model and ONNX Runtime. Ignored by
//! default (CI has neither); run on request:
//!
//! ```text
//! python packaging/runtime_dlls.py fetch
//! set SONARA_KOKORO_MODELS=<folder with kokoro-v1.0.onnx and voices-v1.0.bin>
//! cargo test -p sonara-engine --release --test kokoro_live -- --ignored --nocapture
//! ```
//!
//! `SONARA_ORT_DYLIB` overrides the DLL (default: the fetched one under
//! `target/onnxruntime/`); `SONARA_KOKORO_WAV=<folder>` writes each line as
//! a WAV for listening. Nothing is downloaded: the test copies the model
//! files into a temp folder of its own.
#![cfg(all(windows, feature = "kokoro"))]
mod common;

use sonara_engine::kokoro::download::{pinned_files, Backoff, BASE_URL, MODEL_FILE, VOICES_FILE};
use sonara_engine::kokoro::{Config, Kokoro, SAMPLE_RATE};
use sonara_engine::{Engine, PcmChunk};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn peak_ram_mb() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        rest: [usize; 6],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(h: isize, p: *mut Counters, cb: u32) -> i32;
    }
    let mut c = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    c.peak_working_set as f64 / 1e6
}

const LINES: &[&str] = &[
    "Call get user id before you open the SessionChannel, otherwise the router drops the event.",
    "I changed src/sonara/daemon/ingest.py and tests/test_router.py, then reran the suite.",
    "CI is green on the PR, and the API now returns JSON instead of XML.",
    "Pi is roughly 3.14, and the plugin is now at v0.8.5 after 1689 tests passed.",
    "1: Run sonara keymap --reset, then press Ctrl+Alt+M once to check the mute cycle.",
    "Set SONARA_HOME to %LOCALAPPDATA%\\Sonara and restart sonarad.exe on port 8765.",
    "Use npm i @sonara/client, then call connect() and onState() from the Electron main process.",
    "The ort 2.0 rc crate wraps onnxruntime.dll, and cargo-deny bans espeak-ng and piper-phonemize.",
    // A long reply (M0 corpus line 12): many sentence batches in one call.
    "The policy was never actually active. Here's what happened: I wrote it to the registry on \
     August 28th with the CPU affinity mask, but it only takes effect at boot, and we intentionally \
     skipped rebooting because you wanted a full-severity capture first. Then today when the driver \
     installed, it wiped the policy before the machine ever rebooted. So it was written, sat inert, \
     and got deleted, nothing ever actually moved. Current state: DevicePolicy is empty, and my \
     traces from a few minutes ago show GPU DPCs still landing entirely on CPU 1. Nearly a million \
     microseconds for nvlddmkm, over 700 thousand for dxgkrnl, essentially nothing on all the other \
     30 cores. My recommendation: leave the affinity fix off for now. The data shows the problem is \
     wider than just GPU, afd.sys, tcpip.sys, and storport.sys are all producing DPCs over 512 \
     microseconds.",
];

#[test]
#[ignore = "live: needs the Kokoro model files (SONARA_KOKORO_MODELS) and onnxruntime.dll"]
fn kokoro_speaks_with_the_real_model() {
    let source = PathBuf::from(
        std::env::var("SONARA_KOKORO_MODELS")
            .expect("set SONARA_KOKORO_MODELS to the model folder"),
    );
    let runtime = std::env::var("SONARA_ORT_DYLIB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/onnxruntime/1.28.2/onnxruntime.dll")
        });
    let tmp = common::TempDir::new("live");
    let dir = tmp.path().join("kokoro/v1.0");
    std::fs::create_dir_all(&dir).unwrap();
    for f in [MODEL_FILE, VOICES_FILE] {
        std::fs::copy(source.join(f), dir.join(f)).unwrap();
    }
    let mut config = Config::new(dir, runtime);
    config.files = pinned_files(BASE_URL);
    config.download = false;
    config.backoff = Backoff::default();
    let e = Kokoro::new(config);

    let t = Instant::now();
    e.warm().unwrap();
    assert!(e.is_ready(), "{:?}", e.status());
    println!("verify + load: {:.3} s", t.elapsed().as_secs_f64());

    let t = Instant::now();
    let warm: Vec<PcmChunk> = e
        .synthesize("Ready.", "", 200)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    println!("first call: {:.3} s", t.elapsed().as_secs_f64());
    assert!(!warm.is_empty());

    let one = "CI is green on the PR, and the API now returns JSON instead of XML.";
    for _ in 0..3 {
        let t = Instant::now();
        let mut s = e.synthesize(one, "af_heart", 200).unwrap();
        let first = s.next().unwrap().unwrap();
        println!(
            "first audio, one sentence: {:.3} s",
            t.elapsed().as_secs_f64()
        );
        assert_eq!(first.sample_rate, SAMPLE_RATE);
    }

    let wav_dir = std::env::var("SONARA_KOKORO_WAV").ok().map(PathBuf::from);
    let (mut synth, mut audio) = (Duration::ZERO, 0f64);
    for (i, line) in LINES.iter().enumerate() {
        let t = Instant::now();
        let pcm: Vec<PcmChunk> = e
            .synthesize(line, "af_heart", 250)
            .unwrap()
            .map(Result::unwrap)
            .collect();
        synth += t.elapsed();
        let samples: Vec<i16> = pcm.iter().flat_map(|c| c.samples.iter().copied()).collect();
        audio += samples.len() as f64 / SAMPLE_RATE as f64;
        assert!(
            samples.len() > SAMPLE_RATE as usize / 2,
            "line {i} is too short"
        );
        if let Some(d) = &wav_dir {
            std::fs::create_dir_all(d).unwrap();
            let chunk = PcmChunk {
                samples,
                sample_rate: SAMPLE_RATE,
                channels: 1,
            };
            std::fs::write(
                d.join(format!("{:02}.wav", i + 1)),
                sonara_engine::wav::encode(&chunk),
            )
            .unwrap();
        }
    }
    let rtf = synth.as_secs_f64() / audio;
    println!(
        "corpus: synth {:.2} s, audio {:.2} s, RTF {:.3}, peak working set {:.0} MB",
        synth.as_secs_f64(),
        audio,
        rtf,
        peak_ram_mb()
    );
    assert!(rtf < 0.6, "RTF {rtf}");
}
