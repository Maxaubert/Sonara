//! Earcons: short tones that mark agent events (a question, a permission
//! prompt, the end of a turn...). The WAVs are `sounds/<kind>.wav` in this
//! crate: original procedural sounds (MIT) rendered by
//! `packaging/sounds/build_earcons.py` (#211; `sounds/SHA256SUMS` pins
//! them), compiled in, and played through the L1 output's clip mixer so
//! they never pause or cut speech. `nav_edge` is the same sound as `nav`.
//!
//! **Custom earcons** (`Library`): a host gives a folder; `<kind>.wav` there
//! (`session_change.wav`, `turn_done.wav`, ...) replaces that kind's bundled
//! clip. Any WAV `sonara_engine::wav` decodes is accepted (8/16/24/32-bit
//! integer or 32/64-bit float, mono or stereo, any sample rate): it is
//! mixed down to mono 16-bit and keeps its sample rate, since the clip
//! mixer resamples to the device. The file is checked again (size and
//! modification time) each time its kind plays and on `custom()`, so
//! adding, replacing or deleting a file applies at the next play without a
//! restart. A file that cannot be used (unreadable, not a WAV, silent,
//! empty, longer than `MAX_SECONDS` or bigger than `MAX_BYTES`) falls back
//! to the bundled clip with one log line per version of the file.
use sonara_engine::{wav, PcmChunk};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

/// Longest custom earcon, in seconds (a cue, not a song).
pub const MAX_SECONDS: f32 = 10.0;
/// Biggest custom earcon file, in bytes.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;

macro_rules! earcons {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// The earcon kinds (spec section 8).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Earcon {
            $($variant),*
        }

        impl Earcon {
            pub const ALL: &'static [Earcon] = &[$(Earcon::$variant),*];

            /// The protocol name (`earcon {kind}`).
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Earcon::$variant => $name),*
                }
            }

            fn wav(&self) -> &'static [u8] {
                match self {
                    $(Earcon::$variant => include_bytes!(concat!(
                        "../sounds/",
                        $name,
                        ".wav"
                    ))),*
                }
            }
        }
    };
}

earcons! {
    Choice => "choice",
    Permission => "permission",
    Error => "error",
    TurnDone => "turn_done",
    Nav => "nav",
    NavEdge => "nav_edge",
    SessionChange => "session_change",
    SummaryFailed => "summary_failed",
}

impl Earcon {
    pub fn parse(name: &str) -> Option<Earcon> {
        Earcon::ALL.iter().copied().find(|e| e.as_str() == name)
    }

    /// The decoded clip, mono 16-bit (decoded once).
    pub fn clip(&self) -> &'static PcmChunk {
        static CLIPS: OnceLock<Vec<PcmChunk>> = OnceLock::new();
        let clips = CLIPS.get_or_init(|| {
            Earcon::ALL
                .iter()
                .map(|e| {
                    let pcm = wav::decode(e.wav()).expect("a bundled earcon is a valid WAV");
                    mono(pcm)
                })
                .collect()
        });
        let i = Earcon::ALL
            .iter()
            .position(|e| e == self)
            .expect("every earcon is listed");
        &clips[i]
    }
}

/// Where a log line goes (`Library::new`).
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The version of a file last looked at: modification time and size.
type Version = (Option<SystemTime>, u64);

/// One kind's custom file: the version last looked at and the clip it
/// gave (`None`: unusable).
#[derive(Default)]
struct Slot {
    seen: Option<Version>,
    clip: Option<Arc<PcmChunk>>,
}

/// The earcons in force: the bundled clips, with the custom files of a
/// folder in front (module docs). Shared by every player.
pub struct Library {
    dir: Option<PathBuf>,
    log: Option<Log>,
    slots: Mutex<HashMap<Earcon, Slot>>,
}

impl Library {
    /// Only the bundled clips.
    pub fn bundled() -> Library {
        Library {
            dir: None,
            log: None,
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// The bundled clips with `dir`'s `<kind>.wav` files in front. Every
    /// kind is looked at now, so a bad file is logged at startup.
    pub fn new(dir: PathBuf, log: Option<Log>) -> Library {
        let lib = Library {
            dir: Some(dir),
            log,
            slots: Mutex::new(HashMap::new()),
        };
        let _ = lib.custom();
        lib
    }

    /// The custom earcons folder, if any.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The file that overrides `e` (whether it exists or not).
    pub fn path(&self, e: Earcon) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{}.wav", e.as_str())))
    }

    /// The clip to play for `e` now: its custom file if usable, else the
    /// bundled clip.
    pub fn clip(&self, e: Earcon) -> Arc<PcmChunk> {
        static BUNDLED: OnceLock<Vec<Arc<PcmChunk>>> = OnceLock::new();
        if let Some(c) = self.refresh(e) {
            return c;
        }
        let all = BUNDLED.get_or_init(|| {
            Earcon::ALL
                .iter()
                .map(|e| Arc::new(e.clip().clone()))
                .collect()
        });
        let i = Earcon::ALL.iter().position(|x| *x == e).unwrap_or(0);
        all[i].clone()
    }

    /// The kinds a usable custom file overrides now, in `Earcon::ALL`
    /// order.
    pub fn custom(&self) -> Vec<Earcon> {
        Earcon::ALL
            .iter()
            .copied()
            .filter(|e| self.refresh(*e).is_some())
            .collect()
    }

    fn note(&self, line: &str) {
        if let Some(log) = &self.log {
            log(line);
        }
    }

    /// Look at `e`'s file again if it changed; the custom clip in force.
    fn refresh(&self, e: Earcon) -> Option<Arc<PcmChunk>> {
        let path = self.path(e)?;
        let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file());
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        let slot = slots.entry(e).or_default();
        let Some(meta) = meta else {
            let had = slot.clip.take().is_some();
            slot.seen = None;
            if had {
                self.note(&format!(
                    "earcons: {} is gone; using the bundled {} clip",
                    path.display(),
                    e.as_str()
                ));
            }
            return None;
        };
        let version = (meta.modified().ok(), meta.len());
        if slot.seen != Some(version) {
            slot.seen = Some(version);
            slot.clip = match load(&path, meta.len()) {
                Ok(c) => {
                    self.note(&format!(
                        "earcons: using {} for {}",
                        path.display(),
                        e.as_str()
                    ));
                    Some(Arc::new(c))
                }
                Err(why) => {
                    self.note(&format!(
                        "earcons: {}: {why}; using the bundled {} clip",
                        path.display(),
                        e.as_str()
                    ));
                    None
                }
            };
        }
        slot.clip.clone()
    }
}

/// Read and check one custom earcon file.
fn load(path: &Path, len: u64) -> Result<PcmChunk, String> {
    if len > MAX_BYTES {
        return Err(format!("bigger than {} MB", MAX_BYTES / (1024 * 1024)));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read it ({e})"))?;
    let pcm = wav::decode(&bytes).map_err(|e| format!("not a usable WAV ({e})"))?;
    let pcm = mono(pcm);
    if pcm.samples.is_empty() {
        return Err("it has no sound".into());
    }
    let secs = pcm.samples.len() as f32 / pcm.sample_rate as f32;
    if secs > MAX_SECONDS {
        return Err(format!("it lasts {secs:.1} s, longer than {MAX_SECONDS} s"));
    }
    if pcm.samples.iter().all(|&s| s == 0) {
        return Err("it is silent".into());
    }
    Ok(pcm)
}

/// Mix down to one channel (the bundled WAVs are mono already). Stereo is
/// averaged; with more channels the loudest one is kept, since averaging a
/// surround file whose sound sits in one channel would make it much quieter.
fn mono(pcm: PcmChunk) -> PcmChunk {
    let n = pcm.channels.max(1) as usize;
    if n == 1 {
        return pcm;
    }
    let samples = if n == 2 {
        pcm.samples
            .chunks(n)
            .map(|f| (f.iter().map(|&s| s as i32).sum::<i32>() / f.len() as i32) as i16)
            .collect()
    } else {
        let peak = |c: usize| {
            pcm.samples
                .chunks(n)
                .filter_map(|f| f.get(c))
                .map(|s| s.unsigned_abs())
                .max()
                .unwrap_or(0)
        };
        let loudest = (0..n).max_by_key(|&c| peak(c)).unwrap_or(0);
        pcm.samples
            .chunks(n)
            .map(|f| f.get(loudest).copied().unwrap_or(0))
            .collect()
    };
    PcmChunk {
        samples,
        sample_rate: pcm.sample_rate,
        channels: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_earcon_decodes_to_a_short_mono_clip() {
        for e in Earcon::ALL {
            let c = e.clip();
            assert_eq!(c.channels, 1, "{e:?}");
            assert_eq!(c.sample_rate, 48_000, "{e:?}");
            let secs = c.samples.len() as f32 / c.sample_rate as f32;
            assert!(secs > 0.01 && secs < 2.0, "{e:?} lasts {secs} s");
            // Audible, not just non-zero: every pick peaks well above -30 dBFS.
            let peak = c.samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(peak > 1_000, "{e:?} peaks at {peak}");
        }
    }

    #[test]
    fn the_bundled_wavs_are_the_sound_pack_files() {
        // 16-bit mono PCM at 48 kHz, as packaging/sounds/build_earcons.py
        // writes them (wav::decode would also take other formats).
        for e in Earcon::ALL {
            let b = e.wav();
            assert_eq!(&b[..4], b"RIFF", "{e:?}");
            assert_eq!(u16::from_le_bytes([b[20], b[21]]), 1, "{e:?} format");
            assert_eq!(u16::from_le_bytes([b[22], b[23]]), 1, "{e:?} channels");
            assert_eq!(
                u32::from_le_bytes([b[24], b[25], b[26], b[27]]),
                48_000,
                "{e:?} rate"
            );
            assert_eq!(u16::from_le_bytes([b[34], b[35]]), 16, "{e:?} bits");
        }
        // nav_edge is the same sound as nav (#211), in its own file so a
        // custom nav.wav does not change nav_edge.
        assert_eq!(Earcon::NavEdge.wav(), Earcon::Nav.wav());
        assert_ne!(Earcon::Error.wav(), Earcon::SummaryFailed.wav());
    }

    #[test]
    fn names_round_trip() {
        for e in Earcon::ALL {
            assert_eq!(Earcon::parse(e.as_str()), Some(*e));
        }
        assert_eq!(Earcon::parse("ready"), None);
        assert_eq!(Earcon::TurnDone.as_str(), "turn_done");
    }

    /// A fresh folder in `%TEMP%`, removed when the test ends.
    struct Tmp(PathBuf);

    impl std::ops::Deref for Tmp {
        type Target = PathBuf;
        fn deref(&self) -> &PathBuf {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp() -> Tmp {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "sonara-earcons-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }

    /// A WAV fixture: format `tag` (1 integer PCM, 3 float) of `bits`, on
    /// `channels` channels, `frames` frames of a half-scale square wave.
    pub(crate) fn fixture(tag: u16, bits: u16, channels: u16, rate: u32, frames: usize) -> Vec<u8> {
        let mut data = Vec::new();
        for i in 0..frames {
            let x: f64 = if (i / 20) % 2 == 0 { 0.5 } else { -0.5 };
            for _ in 0..channels {
                match (tag, bits) {
                    (3, 32) => data.extend_from_slice(&(x as f32).to_le_bytes()),
                    (3, _) => data.extend_from_slice(&x.to_le_bytes()),
                    (_, 8) => data.push((128.0 + x * 127.0) as u8),
                    (_, 16) => data.extend_from_slice(&((x * 32767.0) as i16).to_le_bytes()),
                    (_, 24) => {
                        data.extend_from_slice(&((x * 8_388_607.0) as i32).to_le_bytes()[..3])
                    }
                    _ => data.extend_from_slice(&((x * 2_147_483_647.0) as i32).to_le_bytes()),
                }
            }
        }
        let align = u32::from(channels) * u32::from(bits / 8);
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * align).to_le_bytes());
        b.extend_from_slice(&(align as u16).to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(&data);
        b
    }

    fn logged() -> (Log, Arc<Mutex<Vec<String>>>) {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let l = lines.clone();
        (
            Arc::new(move |s: &str| l.lock().unwrap().push(s.to_string())),
            lines,
        )
    }

    #[test]
    fn custom_files_in_several_formats_override_the_bundled_clips() {
        let dir = tmp();
        let cases: [(Earcon, u16, u16, u16, u32); 6] = [
            (Earcon::SessionChange, 1, 16, 2, 48_000),
            (Earcon::TurnDone, 1, 24, 1, 96_000),
            (Earcon::Choice, 1, 32, 2, 22_050),
            (Earcon::Permission, 3, 32, 1, 44_100),
            (Earcon::Error, 3, 64, 2, 8_000),
            (Earcon::Nav, 1, 8, 1, 11_025),
        ];
        for (e, tag, bits, ch, rate) in cases {
            let frames = rate as usize / 10;
            std::fs::write(
                dir.join(format!("{}.wav", e.as_str())),
                fixture(tag, bits, ch, rate, frames),
            )
            .unwrap();
        }
        let (log, lines) = logged();
        let lib = Library::new(dir.to_path_buf(), Some(log));
        assert_eq!(
            lib.custom(),
            [
                Earcon::Choice,
                Earcon::Permission,
                Earcon::Error,
                Earcon::TurnDone,
                Earcon::Nav,
                Earcon::SessionChange
            ]
        );
        for (e, _, bits, _, rate) in cases {
            let c = lib.clip(e);
            assert_eq!((c.channels, c.sample_rate), (1, rate), "{e:?}");
            assert_eq!(c.samples.len(), rate as usize / 10, "{e:?} frames");
            let peak = c.samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(
                (16_000..=16_500).contains(&peak),
                "{e:?} {bits} bit: {peak}"
            );
        }
        // The kinds without a file keep the bundled clip.
        assert_eq!(*lib.clip(Earcon::NavEdge), *Earcon::NavEdge.clip());
        assert_eq!(lines.lock().unwrap().len(), 6, "one line per file loaded");
    }

    #[test]
    fn a_bad_file_falls_back_with_one_log_line_and_changes_apply_at_the_next_play() {
        let dir = tmp();
        let path = dir.join("session_change.wav");
        std::fs::write(&path, b"not a wav").unwrap();
        let (log, lines) = logged();
        let lib = Library::new(dir.to_path_buf(), Some(log));
        assert!(lib.custom().is_empty());
        assert_eq!(
            *lib.clip(Earcon::SessionChange),
            *Earcon::SessionChange.clip()
        );
        {
            let l = lines.lock().unwrap();
            assert_eq!(l.len(), 1, "{l:?}");
            assert!(
                l[0].contains("session_change.wav") && l[0].contains("bundled"),
                "{l:?}"
            );
        }
        // Replaced with a good file: used at the next play.
        std::fs::write(&path, fixture(1, 16, 1, 16_000, 1600)).unwrap();
        assert_eq!(lib.clip(Earcon::SessionChange).sample_rate, 16_000);
        assert_eq!(lib.custom(), [Earcon::SessionChange]);
        // Removed: the bundled clip again.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            *lib.clip(Earcon::SessionChange),
            *Earcon::SessionChange.clip()
        );
        assert!(lib.custom().is_empty());
        // Silent, and too long, are refused too (each a new size, so a new
        // version whatever the clock's resolution).
        let silent = PcmChunk {
            samples: vec![0; 800],
            sample_rate: 8000,
            channels: 1,
        };
        std::fs::write(&path, wav::encode(&silent)).unwrap();
        assert!(lib.custom().is_empty());
        let long = PcmChunk {
            samples: vec![100; 8000 * 11],
            sample_rate: 8000,
            channels: 1,
        };
        std::fs::write(&path, wav::encode(&long)).unwrap();
        assert!(lib.custom().is_empty());
        let l = lines.lock().unwrap();
        assert!(l.iter().any(|l| l.contains("silent")), "{l:?}");
        assert!(l.iter().any(|l| l.contains("longer than")), "{l:?}");
    }

    #[test]
    fn the_bundled_library_has_no_folder() {
        let lib = Library::bundled();
        assert!(lib.dir().is_none() && lib.custom().is_empty());
        assert_eq!(*lib.clip(Earcon::Choice), *Earcon::Choice.clip());
    }

    #[test]
    fn stereo_is_mixed_down() {
        let pcm = PcmChunk {
            samples: vec![10, 20, -4, 4],
            sample_rate: 8_000,
            channels: 2,
        };
        assert_eq!(mono(pcm).samples, [15, 0]);
    }

    #[test]
    fn more_than_two_channels_keep_the_loudest_channel_at_full_level() {
        // A 5.1 file with the chime only in the centre channel (index 2):
        // averaging would cut it to a sixth.
        let mut samples = Vec::new();
        for i in 0..100 {
            let x: i16 = if i % 2 == 0 { 6000 } else { -6000 };
            samples.extend_from_slice(&[0, 0, x, 0, 10, 0]);
        }
        let pcm = PcmChunk {
            samples,
            sample_rate: 48_000,
            channels: 6,
        };
        let m = mono(pcm);
        assert_eq!(m.channels, 1);
        assert_eq!(m.samples.len(), 100);
        assert_eq!(m.samples[0], 6000);
        assert_eq!(m.samples[1], -6000);
    }
}
