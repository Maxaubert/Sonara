//! A silent output that keeps real time (`--output null`): each chunk
//! "plays" for the length of its PCM, honouring pause, resume and stop, and
//! reports the same events as the device output. For the conformance suite
//! and CI runners without an audio device; with the fake engine a chunk
//! lasts 10 ms per character at rate 200.
use sonara_audio::{AudioEvent, ItemId, Output, PcmChunk};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

enum Cmd {
    /// `open`: more comes (`Append`) until `Finish` (#235).
    Play {
        gen: u64,
        length: Duration,
        open: bool,
    },
    Append {
        gen: u64,
        length: Duration,
    },
    Finish {
        gen: u64,
    },
    Pause,
    Resume,
    Stop,
}

pub struct NullOutput {
    cmds: Sender<Cmd>,
}

struct Loaded {
    gen: u64,
    /// Time left to play, as of `since` (or as of the pause).
    left: Duration,
    /// Set while playing.
    since: Option<Instant>,
    /// More audio may come: it does not finish when it runs dry.
    open: bool,
}

impl Loaded {
    /// Played all it has so far.
    fn dry(&self) -> bool {
        self.since.is_some_and(|s| s.elapsed() >= self.left)
    }
}

impl NullOutput {
    pub fn new() -> (Self, Receiver<AudioEvent>) {
        let (cmds, rx) = channel();
        let (events, events_rx) = channel();
        std::thread::Builder::new()
            .name("sonarad-null-output".into())
            .spawn(move || run(rx, events))
            .expect("spawn the null output thread");
        (NullOutput { cmds }, events_rx)
    }
}

fn run(rx: Receiver<Cmd>, events: Sender<AudioEvent>) {
    let mut loaded: Option<Loaded> = None;
    loop {
        let wait = match &loaded {
            // An open chunk that ran dry waits for more (or its finish).
            Some(l) if l.open && l.dry() => Duration::from_secs(3600),
            Some(Loaded {
                left,
                since: Some(since),
                ..
            }) => left.saturating_sub(since.elapsed()),
            _ => Duration::from_secs(3600),
        };
        match rx.recv_timeout(wait) {
            Ok(Cmd::Play { gen, length, open }) => {
                loaded = Some(Loaded {
                    gen,
                    left: length,
                    since: Some(Instant::now()),
                    open,
                });
                let _ = events.send(AudioEvent::ChunkStarted { gen });
            }
            Ok(Cmd::Append { gen, length }) => {
                if let Some(l) = loaded.as_mut().filter(|l| l.gen == gen) {
                    if l.dry() {
                        // Silence meanwhile: play the new audio from now.
                        l.since = Some(Instant::now());
                        l.left = length;
                    } else {
                        l.left += length;
                    }
                }
            }
            Ok(Cmd::Finish { gen }) => {
                if let Some(l) = loaded.as_mut().filter(|l| l.gen == gen) {
                    l.open = false;
                    if l.dry() {
                        let _ = events.send(AudioEvent::ChunkFinished { gen });
                        loaded = None;
                    }
                }
            }
            Ok(Cmd::Pause) => {
                if let Some(l) = loaded.as_mut() {
                    if let Some(since) = l.since.take() {
                        l.left = l.left.saturating_sub(since.elapsed());
                    }
                }
            }
            Ok(Cmd::Resume) => {
                if let Some(l) = loaded.as_mut() {
                    if l.since.is_none() {
                        l.since = Some(Instant::now());
                    }
                }
            }
            Ok(Cmd::Stop) => loaded = None,
            Err(RecvTimeoutError::Timeout) => {
                if let Some(l) = loaded.as_ref().filter(|l| !l.open) {
                    if let Some(since) = l.since {
                        if since.elapsed() >= l.left {
                            let _ = events.send(AudioEvent::ChunkFinished { gen: l.gen });
                            loaded = None;
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn length(pcm: &[PcmChunk]) -> Duration {
    Duration::from_millis(pcm.iter().map(PcmChunk::duration_ms).sum())
}

impl Output for NullOutput {
    fn play(&mut self, pcm: Vec<PcmChunk>, _item: ItemId, _chunk_index: usize, gen: u64) {
        let _ = self.cmds.send(Cmd::Play {
            gen,
            length: length(&pcm),
            open: false,
        });
    }

    fn play_open(&mut self, pcm: Vec<PcmChunk>, _item: ItemId, _chunk_index: usize, gen: u64) {
        let _ = self.cmds.send(Cmd::Play {
            gen,
            length: length(&pcm),
            open: true,
        });
    }

    fn append(&mut self, gen: u64, pcm: Vec<PcmChunk>) {
        let _ = self.cmds.send(Cmd::Append {
            gen,
            length: length(&pcm),
        });
    }

    fn finish(&mut self, gen: u64) {
        let _ = self.cmds.send(Cmd::Finish { gen });
    }

    fn pause(&mut self) {
        let _ = self.cmds.send(Cmd::Pause);
    }

    fn resume(&mut self) {
        let _ = self.cmds.send(Cmd::Resume);
    }

    fn stop(&mut self) {
        let _ = self.cmds.send(Cmd::Stop);
    }

    fn set_volume(&mut self, _percent: u8) {}

    fn play_clip(&mut self, _samples: &[i16], _sample_rate: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(ms: u64) -> Vec<PcmChunk> {
        vec![PcmChunk {
            samples: vec![0; (16 * ms) as usize],
            sample_rate: 16_000,
            channels: 1,
        }]
    }

    #[test]
    fn a_chunk_starts_at_once_and_finishes_after_its_length() {
        let (mut out, rx) = NullOutput::new();
        let t = Instant::now();
        out.play(pcm(100), ItemId(1), 0, 7);
        assert_eq!(rx.recv().unwrap(), AudioEvent::ChunkStarted { gen: 7 });
        assert_eq!(rx.recv().unwrap(), AudioEvent::ChunkFinished { gen: 7 });
        assert!(t.elapsed() >= Duration::from_millis(100));
    }

    #[test]
    fn an_open_chunk_finishes_only_after_finish_and_its_audio() {
        let (mut out, rx) = NullOutput::new();
        let t = Instant::now();
        out.play_open(pcm(50), ItemId(1), 0, 3);
        assert_eq!(rx.recv().unwrap(), AudioEvent::ChunkStarted { gen: 3 });
        // Dry after 50 ms, but open: no finish.
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        out.append(3, pcm(60));
        out.append(9, pcm(5000)); // another chunk's: ignored
        out.finish(3);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            AudioEvent::ChunkFinished { gen: 3 }
        );
        assert!(t.elapsed() >= Duration::from_millis(250));
        assert!(
            t.elapsed() < Duration::from_secs(4),
            "the other gen was ignored"
        );
        // A finish while still playing: done at the end of the audio.
        out.play_open(pcm(100), ItemId(1), 1, 4);
        out.finish(4);
        assert_eq!(rx.recv().unwrap(), AudioEvent::ChunkStarted { gen: 4 });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            AudioEvent::ChunkFinished { gen: 4 }
        );
    }

    #[test]
    fn pause_holds_and_stop_discards() {
        let (mut out, rx) = NullOutput::new();
        out.play(pcm(50), ItemId(1), 0, 1);
        out.pause();
        assert_eq!(rx.recv().unwrap(), AudioEvent::ChunkStarted { gen: 1 });
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        out.resume();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            AudioEvent::ChunkFinished { gen: 1 }
        );
        out.play(pcm(50), ItemId(1), 1, 2);
        out.stop();
        assert_eq!(rx.recv().unwrap(), AudioEvent::ChunkStarted { gen: 2 });
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }
}
