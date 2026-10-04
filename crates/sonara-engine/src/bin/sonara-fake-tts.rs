//! A stand-in speech program for the tests of the `command` kind (feature
//! `test-util`; never shipped). It reads the text from `--text` or stdin and
//! answers as told:
//!
//! - `--mode wav|pcm|file|garbage|empty` (default `wav`): a WAV or raw PCM
//!   on stdout, a WAV at `--out`, text that is no audio, or nothing.
//! - `--rate N`: the sample rate (default 22050).
//! - `--sleep-ms N`, then `--marker PATH` is written (to see a kill).
//! - `--exit N`: print two lines to stderr and exit with N.
//! - `--record PATH`: write `{"args", "stdin", "key"}` as JSON there.
//!
//! The samples are `[characters of the text, 100, -100]`.
use sonara_engine::{wav, PcmChunk};
use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let text = match value("--text") {
        Some(t) => t,
        None => {
            let mut s = String::new();
            let _ = std::io::stdin().read_to_string(&mut s);
            s
        }
    };
    if let Some(path) = value("--record") {
        let record = serde_json::json!({
            "args": args,
            "stdin": if value("--text").is_some() { String::new() } else { text.clone() },
            "key": std::env::var("SONARA_ENGINE_KEY").ok(),
        });
        std::fs::write(path, record.to_string()).unwrap();
    }
    if let Some(ms) = value("--sleep-ms").and_then(|v| v.parse::<u64>().ok()) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if let Some(path) = value("--marker") {
        std::fs::write(path, b"done").unwrap();
    }
    if let Some(code) = value("--exit").and_then(|v| v.parse::<i32>().ok()) {
        eprintln!("fake-tts: loading the voice");
        eprintln!("fake-tts: the model broke, key sk-abcdefghijklmnop0123456789");
        eprintln!();
        std::process::exit(code);
    }
    let rate = value("--rate")
        .and_then(|v| v.parse().ok())
        .unwrap_or(22_050);
    let samples = vec![text.chars().count() as i16, 100, -100];
    let wav_bytes = || {
        wav::encode(&PcmChunk {
            samples: samples.clone(),
            sample_rate: rate,
            channels: 1,
        })
    };
    let mut out = std::io::stdout();
    match value("--mode").as_deref().unwrap_or("wav") {
        "pcm" => {
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            out.write_all(&bytes).unwrap();
        }
        "file" => {
            std::fs::write(value("--out").expect("--out"), wav_bytes()).unwrap();
        }
        "garbage" => out.write_all(b"Usage: tts [options]\n").unwrap(),
        "empty" => {}
        _ => out.write_all(&wav_bytes()).unwrap(),
    }
}
