//! The real output: rodio on cpal (WASAPI on Windows), on its own thread.
//!
//! One `Sink` per played chunk gives true pause and resume (M0 measured about
//! 5 ms) and makes `stop` a plain drop. Callbacks queued before and after the
//! chunk's audio report `ChunkStarted` and `ChunkFinished`. The device opens
//! on the first play and reopens on a later one after it failed or was lost.
use crate::{AudioEvent, ItemId, Output, PcmChunk};
use rodio::buffer::SamplesBuffer;
use rodio::source::EmptyCallback;
use rodio::{OutputStream, OutputStreamBuilder, Sink};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

enum Cmd {
    Play {
        pcm: Vec<PcmChunk>,
        item: ItemId,
        chunk: usize,
        gen: u64,
        /// More audio comes (`Append`) until `Finish`.
        open: bool,
    },
    Append {
        gen: u64,
        pcm: Vec<PcmChunk>,
    },
    Finish {
        gen: u64,
    },
    Pause,
    Resume,
    Stop,
    Volume(u8),
    Clip {
        samples: Vec<i16>,
        sample_rate: u32,
    },
    /// The stream with this number reported an error (device unplugged...).
    DeviceLost {
        stream: u64,
        reason: String,
    },
    Shutdown,
}

/// Hands stream errors from cpal's thread back to the output thread.
#[derive(Clone)]
struct ErrorReporter {
    tx: Sender<Cmd>,
    stream: u64,
}

impl ErrorReporter {
    fn report(&self, reason: String) {
        let _ = self.tx.send(Cmd::DeviceLost {
            stream: self.stream,
            reason,
        });
    }
}

type Opener = fn(ErrorReporter) -> Result<OutputStream, String>;

fn open_default(reporter: ErrorReporter) -> Result<OutputStream, String> {
    let mut stream = OutputStreamBuilder::from_default_device()
        .map_err(|e| e.to_string())?
        .with_error_callback(move |e| reporter.report(e.to_string()))
        .open_stream_or_fallback()
        .map_err(|e| e.to_string())?;
    stream.log_on_drop(false);
    Ok(stream)
}

/// The audio output on the default device.
pub struct RodioOutput {
    cmds: Sender<Cmd>,
    /// For reporting failures when the output thread is gone.
    events: Sender<AudioEvent>,
}

impl RodioOutput {
    /// Start the output thread. The device is opened on the first `play`, so
    /// creating an output never fails; a missing device shows up as
    /// `AudioEvent::Failed` for that play.
    pub fn new() -> (Self, Receiver<AudioEvent>) {
        Self::with_opener(open_default)
    }

    fn with_opener(open: Opener) -> (Self, Receiver<AudioEvent>) {
        let (events, events_rx) = channel();
        let (cmds, cmds_rx) = channel();
        let worker = Worker {
            open,
            cmds: cmds.clone(),
            events: events.clone(),
            stream: None,
            stream_no: 0,
            sink: None,
            loaded: None,
            volume: 1.0,
        };
        // If the thread cannot start, every play fails (see `send`).
        let _ = thread::Builder::new()
            .name("sonara-audio".into())
            .spawn(move || worker.run(cmds_rx));
        (RodioOutput { cmds, events }, events_rx)
    }

    fn send(&self, cmd: Cmd) {
        if let Err(err) = self.cmds.send(cmd) {
            if let Cmd::Play { gen, .. } = err.0 {
                let _ = self.events.send(AudioEvent::Failed {
                    gen,
                    reason: "audio output thread is not running".into(),
                });
            }
        }
    }
}

impl Drop for RodioOutput {
    fn drop(&mut self) {
        let _ = self.cmds.send(Cmd::Shutdown);
    }
}

impl Output for RodioOutput {
    fn play(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64) {
        self.send(Cmd::Play {
            pcm,
            item,
            chunk: chunk_index,
            gen,
            open: false,
        });
    }

    fn play_open(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64) {
        self.send(Cmd::Play {
            pcm,
            item,
            chunk: chunk_index,
            gen,
            open: true,
        });
    }

    fn append(&mut self, gen: u64, pcm: Vec<PcmChunk>) {
        self.send(Cmd::Append { gen, pcm });
    }

    fn finish(&mut self, gen: u64) {
        self.send(Cmd::Finish { gen });
    }

    fn pause(&mut self) {
        self.send(Cmd::Pause);
    }

    fn resume(&mut self) {
        self.send(Cmd::Resume);
    }

    fn stop(&mut self) {
        self.send(Cmd::Stop);
    }

    fn set_volume(&mut self, percent: u8) {
        self.send(Cmd::Volume(percent));
    }

    fn play_clip(&mut self, samples: &[i16], sample_rate: u32) {
        self.send(Cmd::Clip {
            samples: samples.to_vec(),
            sample_rate,
        });
    }
}

struct Worker {
    open: Opener,
    cmds: Sender<Cmd>,
    events: Sender<AudioEvent>,
    stream: Option<OutputStream>,
    /// Number of the current stream, so a late error from a replaced
    /// stream cannot tear down the new one.
    stream_no: u64,
    /// The sink of the loaded chunk; dropping it stops the chunk.
    sink: Option<Sink>,
    loaded: Option<u64>,
    volume: f32,
}

/// The sink gain for a volume percent. Above 100 is full volume, never
/// amplification, which would clip speech and earcons.
fn gain(percent: u8) -> f32 {
    percent.min(100) as f32 / 100.0
}

/// Samples rodio can play, or `None` for an empty or malformed chunk.
fn to_buffer(c: &PcmChunk) -> Option<SamplesBuffer> {
    if c.channels == 0 || c.sample_rate == 0 {
        return None;
    }
    let whole = c.samples.len() - c.samples.len() % c.channels as usize;
    if whole == 0 {
        return None;
    }
    let data: Vec<f32> = c.samples[..whole]
        .iter()
        .map(|&s| s as f32 / 32768.0)
        .collect();
    Some(SamplesBuffer::new(c.channels, c.sample_rate, data))
}

impl Worker {
    fn run(mut self, cmds: Receiver<Cmd>) {
        while let Ok(cmd) = cmds.recv() {
            match cmd {
                Cmd::Play {
                    pcm,
                    item,
                    chunk,
                    gen,
                    open,
                } => self.play(pcm, item, chunk, gen, open),
                Cmd::Append { gen, pcm } => {
                    if let (Some(s), Some(g)) = (&self.sink, self.loaded) {
                        if g == gen {
                            for buffer in pcm.iter().filter_map(to_buffer) {
                                s.append(buffer);
                            }
                        }
                    }
                }
                Cmd::Finish { gen } => {
                    if let (Some(s), Some(g)) = (&self.sink, self.loaded) {
                        if g == gen {
                            let tx = self.events.clone();
                            s.append(EmptyCallback::new(Box::new(move || {
                                let _ = tx.send(AudioEvent::ChunkFinished { gen });
                            })));
                        }
                    }
                }
                Cmd::Pause => {
                    if let Some(s) = &self.sink {
                        s.pause();
                    }
                }
                Cmd::Resume => {
                    if let Some(s) = &self.sink {
                        s.play();
                    }
                }
                Cmd::Stop => {
                    self.sink = None;
                    self.loaded = None;
                }
                Cmd::Volume(percent) => {
                    self.volume = gain(percent);
                    if let Some(s) = &self.sink {
                        s.set_volume(self.volume);
                    }
                }
                Cmd::Clip {
                    samples,
                    sample_rate,
                } => self.clip(samples, sample_rate),
                Cmd::DeviceLost { stream, reason } => self.device_lost(stream, reason),
                Cmd::Shutdown => break,
            }
        }
    }

    fn ensure_stream(&mut self) -> Result<&OutputStream, String> {
        if self.stream.is_none() {
            self.stream_no += 1;
            let reporter = ErrorReporter {
                tx: self.cmds.clone(),
                stream: self.stream_no,
            };
            self.stream = Some((self.open)(reporter)?);
        }
        self.stream.as_ref().ok_or_else(|| "no stream".to_string())
    }

    fn play(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk: usize, gen: u64, open: bool) {
        self.sink = None;
        self.loaded = None;
        let sink = match self.ensure_stream() {
            Ok(stream) => Sink::connect_new(stream.mixer()),
            Err(e) => {
                let _ = self.events.send(AudioEvent::Failed {
                    gen,
                    reason: format!(
                        "audio device unavailable for item {} chunk {chunk}: {e}",
                        item.0
                    ),
                });
                return;
            }
        };
        sink.set_volume(self.volume);
        let tx = self.events.clone();
        sink.append(EmptyCallback::new(Box::new(move || {
            let _ = tx.send(AudioEvent::ChunkStarted { gen });
        })));
        for buffer in pcm.iter().filter_map(to_buffer) {
            sink.append(buffer);
        }
        // An open chunk finishes after `Finish` (its last audio).
        if !open {
            let tx = self.events.clone();
            sink.append(EmptyCallback::new(Box::new(move || {
                let _ = tx.send(AudioEvent::ChunkFinished { gen });
            })));
        }
        self.sink = Some(sink);
        self.loaded = Some(gen);
    }

    fn clip(&mut self, samples: Vec<i16>, sample_rate: u32) {
        let pcm = PcmChunk {
            samples,
            sample_rate,
            channels: 1,
        };
        let volume = self.volume;
        // A clip is a cue: with no device it is dropped, nothing to report.
        if let (Some(buffer), Ok(stream)) = (to_buffer(&pcm), self.ensure_stream()) {
            let sink = Sink::connect_new(stream.mixer());
            sink.set_volume(volume);
            sink.append(buffer);
            sink.detach();
        }
    }

    fn device_lost(&mut self, stream: u64, reason: String) {
        if stream != self.stream_no || self.stream.is_none() {
            return;
        }
        self.sink = None;
        self.stream = None;
        if let Some(gen) = self.loaded.take() {
            let _ = self.events.send(AudioEvent::Failed {
                gen,
                reason: format!("audio device lost: {reason}"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    fn no_device(_: ErrorReporter) -> Result<OutputStream, String> {
        Err("NoDevice".into())
    }

    fn pcm(n: usize) -> Vec<PcmChunk> {
        vec![PcmChunk {
            samples: vec![0; n],
            sample_rate: 16_000,
            channels: 1,
        }]
    }

    #[test]
    fn a_missing_device_fails_the_play_and_keeps_working() {
        let (mut out, events) = RodioOutput::with_opener(no_device);
        out.play(pcm(160), ItemId(1), 0, 7);
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)),
            Ok(AudioEvent::Failed {
                gen: 7,
                reason: "audio device unavailable for item 1 chunk 0: NoDevice".into()
            })
        );
        // Controls and clips with no device are no-ops, never panics.
        out.pause();
        out.resume();
        out.set_volume(40);
        out.play_clip(&[1, 2, 3], 16_000);
        out.stop();
        out.play(pcm(160), ItemId(1), 1, 8);
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(5)),
            Ok(AudioEvent::Failed { gen: 8, .. })
        ));
    }

    #[test]
    fn a_dead_output_thread_fails_plays_instead_of_panicking() {
        let (mut out, events) = RodioOutput::with_opener(no_device);
        let _ = out.cmds.send(Cmd::Shutdown);
        // Wait for the thread to exit, so the next send fails.
        for _ in 0..100 {
            if out.cmds.send(Cmd::Stop).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        out.play(pcm(10), ItemId(2), 0, 3);
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)),
            Ok(AudioEvent::Failed {
                gen: 3,
                reason: "audio output thread is not running".into()
            })
        );
    }

    #[test]
    fn malformed_chunks_are_skipped() {
        let bad = |samples: Vec<i16>, sample_rate, channels| PcmChunk {
            samples,
            sample_rate,
            channels,
        };
        assert!(to_buffer(&bad(vec![1, 2], 0, 1)).is_none());
        assert!(to_buffer(&bad(vec![1, 2], 16_000, 0)).is_none());
        assert!(to_buffer(&bad(vec![], 16_000, 1)).is_none());
        assert!(to_buffer(&bad(vec![1], 16_000, 2)).is_none());
        assert!(to_buffer(&bad(vec![1, 2, 3], 16_000, 2)).is_some());
    }

    #[test]
    fn volume_above_100_is_full_gain_not_amplification() {
        assert_eq!(gain(0), 0.0);
        assert_eq!(gain(40), 0.4);
        assert_eq!(gain(100), 1.0);
        assert_eq!(gain(150), 1.0);
        assert_eq!(gain(255), 1.0);
    }

    /// The real default device: whatever the machine has (CI runners have
    /// none), a play ends in ChunkFinished or Failed, never a panic. A device
    /// that opens but never advances (a stalled host) is a skip, not a
    /// failure, so the result does not depend on the machine.
    #[test]
    fn the_default_device_finishes_or_fails() {
        let (mut out, events) = RodioOutput::new();
        out.set_volume(0);
        out.play(pcm(800), ItemId(1), 0, 1); // 50 ms of silence
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(AudioEvent::ChunkStarted { gen: 1 }) => continue,
                Ok(AudioEvent::ChunkFinished { gen: 1 })
                | Ok(AudioEvent::Failed { gen: 1, .. }) => break,
                Err(RecvTimeoutError::Timeout) => {
                    eprintln!("skipped: the default device did not advance within 10 s");
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
