//! Live check (#288): the output follows a new default output device at the
//! next chunk and never stalls when the device it plays on goes away.
//!
//! Opt-in, it plays a quiet tone on the real default output for about 20 s:
//!
//!     cargo test -p sonara-audio --test device_follow_live -- --ignored --nocapture
//!
//! While it plays, change the default output (Windows: Sound settings >
//! Output; macOS: System Settings > Sound > Output, or the menu bar), connect
//! or disconnect Bluetooth headphones (AirPods), or unplug a USB or HDMI
//! device. Pass: every chunk ends (finished, or failed for a lost device on
//! Windows), the tone moves to the new device by the next chunk, and the
//! printed log names each device the output opened.
use sonara_audio::{AudioEvent, ItemId, Output, PcmChunk, RodioOutput};
use std::sync::mpsc::channel;
use std::sync::Mutex;
use std::time::Duration;

const RATE: u32 = 24_000;
const CHUNKS: usize = 40;

/// Half a second of a 440 Hz tone at a low level.
fn tone() -> Vec<PcmChunk> {
    let n = RATE as usize / 2;
    let samples = (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 3000.0) as i16
        })
        .collect();
    vec![PcmChunk {
        samples,
        sample_rate: RATE,
        channels: 1,
    }]
}

#[test]
#[ignore = "live: plays audio on the default output; switch devices while it runs"]
fn the_output_follows_a_new_default_device_and_never_stalls() {
    let (log_tx, log) = channel();
    let log_tx = Mutex::new(log_tx);
    let (mut out, events) = RodioOutput::with_log(Box::new(move |line| {
        let _ = log_tx.lock().unwrap().send(line.to_string());
    }));
    out.set_volume(30);
    eprintln!("Playing {CHUNKS} half-second chunks: switch the default output now.");
    let mut lost = 0;
    for chunk in 0..CHUNKS {
        let gen = chunk as u64 + 1;
        out.play(tone(), ItemId(1), chunk, gen);
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(AudioEvent::ChunkStarted { gen: g }) if g == gen => continue,
                Ok(AudioEvent::ChunkFinished { gen: g }) if g == gen => break,
                Ok(AudioEvent::Failed { gen: g, reason }) if g == gen => {
                    eprintln!("chunk {chunk}: {reason}");
                    lost += 1;
                    break;
                }
                Ok(_) => continue,
                Err(_) => panic!("chunk {chunk} stalled: no end within 10 s"),
            }
        }
        for line in log.try_iter() {
            eprintln!("chunk {chunk}: {line}");
        }
    }
    eprintln!("done: {lost} chunk(s) failed on a lost device");
}
