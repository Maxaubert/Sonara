//! A debugging aid: with `SONARA_AUDIO_DUMP=<folder>` the output writes every
//! speech chunk and clip it hands to the device as a WAV, with a
//! `timeline.txt` saying when each started. It shows whether a missing
//! sound (a clipped first word) was in the audio sent or lost after it.
use crate::PcmChunk;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

pub struct Dump {
    dir: PathBuf,
    start: Instant,
    n: u32,
    /// The speech chunk being written: its file, samples and rate.
    cur: Option<(PathBuf, Vec<i16>, u32)>,
}

impl Dump {
    /// A dump into `SONARA_AUDIO_DUMP`, when that is set to a folder that
    /// exists or can be made.
    pub fn from_env() -> Option<Dump> {
        let dir = PathBuf::from(std::env::var_os("SONARA_AUDIO_DUMP")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Dump {
            dir,
            start: Instant::now(),
            n: 0,
            cur: None,
        })
    }

    /// One line in `timeline.txt`: milliseconds since the dump began.
    pub fn note(&self, what: &str) {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("timeline.txt"))
        {
            let _ = writeln!(f, "{:>8} ms  {what}", self.start.elapsed().as_millis());
        }
    }

    /// A speech chunk starts (`pcm` is what is queued with it).
    pub fn play(&mut self, item: u64, chunk: usize, pcm: &[PcmChunk]) {
        self.n += 1;
        let rate = pcm.first().map_or(24_000, |c| c.sample_rate);
        let path = self
            .dir
            .join(format!("{:03}-item{item}-chunk{chunk}.wav", self.n));
        self.note(&format!("play  {}", path.display()));
        self.cur = Some((path, Vec::new(), rate));
        self.append(pcm);
    }

    /// More audio for the chunk being written.
    pub fn append(&mut self, pcm: &[PcmChunk]) {
        let Some((path, samples, rate)) = &mut self.cur else {
            return;
        };
        for c in pcm {
            samples.extend_from_slice(&c.samples);
        }
        write_wav(path, samples, *rate);
    }

    /// A clip (an earcon or a cue) starts.
    pub fn clip(&mut self, samples: &[i16], rate: u32) {
        self.n += 1;
        let path = self.dir.join(format!("{:03}-clip.wav", self.n));
        self.note(&format!("clip  {}", path.display()));
        write_wav(&path, samples, rate);
    }
}

fn write_wav(path: &PathBuf, samples: &[i16], rate: u32) {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    let _ = std::fs::write(path, out);
}
