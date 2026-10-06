//! A test output that writes what it is given to WAV files
//! (`--output wav:<DIR>`, #274): it keeps time like the null output (each
//! chunk "plays" for the length of its PCM, honouring pause, resume and
//! stop, with the same events) and writes the PCM of every chunk it is
//! handed to `<DIR>`, so an end-to-end test can check the real audio of a
//! real engine: not silent, a plausible length, the right sample rate.
//!
//! Files, numbered in the order the output got them:
//! - `<seq>-item<item_id>-chunk<index>.wav`: one per chunk played (a chunk
//!   played again, after `previous` or `restart`, gets a new file). A
//!   streamed chunk (`play_open` and `append`) is rewritten as its audio
//!   comes, so the file is complete once the chunk finished.
//! - `<seq>-clip.wav`: a clip (an earcon, a preview, `engine_test` with
//!   `play`).
//!
//! The files hold the PCM as the engine made it (before volume and mute)
//! and all of it, also when a stop cut the chunk. Mono or stereo 16-bit
//! PCM at the engine's rate; audio of another format within one chunk goes
//! to its own file (`...-chunk<index>.<part>.wav`). Never on by default:
//! a testing aid, not for apps.
use crate::null_output::NullOutput;
use sonara_audio::{AudioEvent, ItemId, Output, PcmChunk};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

/// The open (streamed) chunks: gen -> what was written so far.
struct Recording {
    base: String,
    part: u32,
    rate: u32,
    channels: u16,
    samples: Vec<i16>,
}

pub struct WavOutput {
    timing: NullOutput,
    dir: PathBuf,
    seq: u64,
    open: HashMap<u64, Recording>,
}

impl WavOutput {
    /// Creates `dir` if needed.
    pub fn new(dir: &Path) -> Result<(Self, Receiver<AudioEvent>), String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create the WAV output folder {}: {e}", dir.display()))?;
        let (timing, events) = NullOutput::new();
        Ok((
            WavOutput {
                timing,
                dir: dir.to_path_buf(),
                seq: 0,
                open: HashMap::new(),
            },
            events,
        ))
    }

    fn next_base(&mut self, what: &str) -> String {
        self.seq += 1;
        format!("{:05}-{what}", self.seq)
    }

    fn path(&self, base: &str, part: u32) -> PathBuf {
        if part == 0 {
            self.dir.join(format!("{base}.wav"))
        } else {
            self.dir.join(format!("{base}.{part}.wav"))
        }
    }

    /// Writes `pcm` under `base`, one file per run of one format; returns
    /// the recording of the last run (for a chunk that goes on).
    fn write_runs(&self, base: String, pcm: &[PcmChunk]) -> Option<Recording> {
        let mut rec: Option<Recording> = None;
        for c in pcm.iter().filter(|c| c.sample_rate > 0 && c.channels > 0) {
            match rec.as_mut() {
                Some(r) if r.rate == c.sample_rate && r.channels == c.channels => {
                    r.samples.extend_from_slice(&c.samples)
                }
                _ => {
                    let part = match rec.take() {
                        Some(done) => {
                            self.save(&done);
                            done.part + 1
                        }
                        None => 0,
                    };
                    rec = Some(Recording {
                        base: base.clone(),
                        part,
                        rate: c.sample_rate,
                        channels: c.channels,
                        samples: c.samples.clone(),
                    });
                }
            }
        }
        if let Some(r) = &rec {
            self.save(r);
        }
        rec
    }

    fn save(&self, r: &Recording) {
        let path = self.path(&r.base, r.part);
        if let Err(e) = std::fs::write(&path, wav_bytes(&r.samples, r.rate, r.channels)) {
            eprintln!("sonarad: cannot write {}: {e}", path.display());
        }
    }
}

/// A 16-bit PCM WAV file.
pub fn wav_bytes(samples: &[i16], rate: u32, channels: u16) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let block = channels * 2;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block as u32).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

impl Output for WavOutput {
    fn play(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64) {
        let base = self.next_base(&format!("item{}-chunk{chunk_index}", item.0));
        self.write_runs(base, &pcm);
        self.timing.play(pcm, item, chunk_index, gen);
    }

    fn play_open(&mut self, pcm: Vec<PcmChunk>, item: ItemId, chunk_index: usize, gen: u64) {
        let base = self.next_base(&format!("item{}-chunk{chunk_index}", item.0));
        // Written even while empty, so the chunk has its file from the start.
        let rec = self.write_runs(base.clone(), &pcm).unwrap_or(Recording {
            base,
            part: 0,
            rate: 0,
            channels: 0,
            samples: Vec::new(),
        });
        self.open.insert(gen, rec);
        self.timing.play_open(pcm, item, chunk_index, gen);
    }

    fn append(&mut self, gen: u64, pcm: Vec<PcmChunk>) {
        if let Some(mut rec) = self.open.remove(&gen) {
            for c in pcm.iter().filter(|c| c.sample_rate > 0 && c.channels > 0) {
                if rec.rate == 0 {
                    rec.rate = c.sample_rate;
                    rec.channels = c.channels;
                }
                if rec.rate != c.sample_rate || rec.channels != c.channels {
                    self.save(&rec);
                    rec = Recording {
                        base: rec.base.clone(),
                        part: rec.part + 1,
                        rate: c.sample_rate,
                        channels: c.channels,
                        samples: Vec::new(),
                    };
                }
                rec.samples.extend_from_slice(&c.samples);
            }
            if rec.rate != 0 {
                self.save(&rec);
            }
            self.open.insert(gen, rec);
        }
        self.timing.append(gen, pcm);
    }

    fn finish(&mut self, gen: u64) {
        self.open.remove(&gen);
        self.timing.finish(gen);
    }

    fn pause(&mut self) {
        self.timing.pause();
    }

    fn resume(&mut self) {
        self.timing.resume();
    }

    fn stop(&mut self) {
        self.open.clear();
        self.timing.stop();
    }

    fn set_volume(&mut self, percent: u8) {
        self.timing.set_volume(percent);
    }

    fn play_clip(&mut self, samples: &[i16], sample_rate: u32) {
        let base = self.next_base("clip");
        self.save(&Recording {
            base,
            part: 0,
            rate: sample_rate,
            channels: 1,
            samples: samples.to_vec(),
        });
        self.timing.play_clip(samples, sample_rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn pcm(ms: u64, rate: u32, value: i16) -> PcmChunk {
        PcmChunk {
            samples: vec![value; (rate as u64 * ms / 1000) as usize],
            sample_rate: rate,
            channels: 1,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sonarad-wav-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn read(path: &Path) -> (u32, u16, Vec<i16>) {
        let b = std::fs::read(path).unwrap();
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[8..16], b"WAVEfmt ");
        assert_eq!(
            u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize,
            b.len() - 8
        );
        let channels = u16::from_le_bytes(b[22..24].try_into().unwrap());
        let rate = u32::from_le_bytes(b[24..28].try_into().unwrap());
        assert_eq!(&b[36..40], b"data");
        let len = u32::from_le_bytes(b[40..44].try_into().unwrap()) as usize;
        let samples = b[44..44 + len]
            .chunks(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]))
            .collect();
        (rate, channels, samples)
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn each_chunk_played_is_a_wav_file_and_keeps_its_timing() {
        let dir = temp_dir("chunks");
        let (mut out, rx) = WavOutput::new(&dir).unwrap();
        out.play(vec![pcm(50, 24_000, 1000)], ItemId(3), 0, 1);
        assert_eq!(rx.recv().unwrap(), AudioEvent::ChunkStarted { gen: 1 });
        assert_eq!(rx.recv().unwrap(), AudioEvent::ChunkFinished { gen: 1 });
        out.play(vec![pcm(20, 16_000, -5)], ItemId(3), 1, 2);
        out.play_clip(&[7; 160], 16_000);
        assert_eq!(
            files(&dir),
            [
                "00001-item3-chunk0.wav",
                "00002-item3-chunk1.wav",
                "00003-clip.wav"
            ]
        );
        let (rate, ch, s) = read(&dir.join("00001-item3-chunk0.wav"));
        assert_eq!((rate, ch, s.len()), (24_000, 1, 1200));
        assert!(s.iter().all(|&v| v == 1000));
        assert_eq!(read(&dir.join("00002-item3-chunk1.wav")).0, 16_000);
        assert_eq!(read(&dir.join("00003-clip.wav")).2.len(), 160);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_streamed_chunk_grows_until_it_finishes_and_a_new_format_is_a_new_part() {
        let dir = temp_dir("stream");
        let (mut out, rx) = WavOutput::new(&dir).unwrap();
        out.play_open(vec![pcm(10, 24_000, 1)], ItemId(1), 0, 5);
        assert_eq!(rx.recv().unwrap(), AudioEvent::ChunkStarted { gen: 5 });
        out.append(5, vec![pcm(10, 24_000, 2)]);
        out.append(9, vec![pcm(10, 24_000, 3)]); // another gen: ignored
        assert_eq!(read(&dir.join("00001-item1-chunk0.wav")).2.len(), 480);
        out.append(5, vec![pcm(10, 16_000, 4)]);
        out.finish(5);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            AudioEvent::ChunkFinished { gen: 5 }
        );
        assert_eq!(
            files(&dir),
            ["00001-item1-chunk0.1.wav", "00001-item1-chunk0.wav"]
        );
        assert_eq!(read(&dir.join("00001-item1-chunk0.1.wav")).2, vec![4; 160]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
