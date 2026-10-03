//! Earcons: short sounds that mark agent events (a question, a permission
//! prompt, the end of a turn...), played through the L1 output's clip mixer
//! so they never pause or cut speech.
//!
//! **The sound library** (#211): every `<name>.wav` in
//! `crates/sonara-agent/sounds/` is compiled in (`build.rs` scans the
//! folder) and offered for every event; `sounds/defaults.txt` gives each
//! event its default sound, or none (silent). Adding or removing a sound is
//! a file change only.
//!
//! **What plays** for an event (`Library::effective`) follows the user's
//! selection (`Source`): a library sound, their own file (`custom`),
//! silence (`none`), or the default. Without a selection, a usable custom
//! file plays, else the default (the folder override of #209 keeps
//! working: a file there is `custom`).
//!
//! **Custom earcons**: a host gives a folder; `<kind>.wav` there
//! (`session_change.wav`, `turn_done.wav`, ...) is that kind's own sound,
//! dropped in by hand or saved by `Library::save_custom` (the settings
//! page's upload). Any WAV `sonara_engine::wav` decodes is accepted
//! (8/16/24/32-bit integer or 32/64-bit float, mono or stereo, any sample
//! rate): it is mixed down to mono 16-bit and keeps its sample rate, since
//! the clip mixer resamples to the device. The file is checked again (size
//! and modification time) each time its kind plays and on `custom()`, so
//! adding, replacing or deleting a file applies at the next play without a
//! restart. A file that cannot be used (unreadable, not a WAV, silent,
//! empty, longer than `MAX_SECONDS` or bigger than `MAX_BYTES`) is ignored
//! (the default plays) with one log line per version of the file.
use sonara_engine::{wav, PcmChunk};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

mod bundled {
    include!(concat!(env!("OUT_DIR"), "/sounds.rs"));
}

/// Longest custom earcon, in seconds (a cue, not a song).
pub const MAX_SECONDS: f32 = 10.0;
/// Biggest custom earcon file dropped in the folder, in bytes.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Biggest WAV `Library::save_custom` accepts (an upload), in bytes: ten
/// seconds of mono 16-bit audio at 48 kHz fit.
pub const MAX_UPLOAD_BYTES: usize = 1024 * 1024;

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

    /// The bundled sound this kind plays by default (`None`: silent).
    pub fn default_sound(&self) -> Option<&'static str> {
        bundled::DEFAULTS
            .iter()
            .find(|(e, _)| *e == self.as_str())
            .map(|(_, s)| *s)
    }

    /// What plays by default: a library sound or silence.
    pub fn default_source(&self) -> Source {
        match self.default_sound() {
            Some(id) => Source::Library(id.to_string()),
            None => Source::Silent,
        }
    }
}

/// The names of the bundled sounds, sorted.
pub fn sounds() -> Vec<&'static str> {
    bundled::SOUNDS.iter().map(|(id, _)| *id).collect()
}

/// A sound's name for people: `reply-done` is "Reply done".
pub fn sound_label(id: &str) -> String {
    let words = id.replace(['-', '_'], " ");
    let mut c = words.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A bundled sound, decoded once (mono 16-bit), or `None` if there is no
/// sound of that name.
pub fn sound_clip(id: &str) -> Option<Arc<PcmChunk>> {
    static CLIPS: OnceLock<Vec<Arc<PcmChunk>>> = OnceLock::new();
    let clips = CLIPS.get_or_init(|| {
        bundled::SOUNDS
            .iter()
            .map(|(id, bytes)| {
                let pcm = wav::decode(bytes)
                    .unwrap_or_else(|e| panic!("the bundled sound {id} is not a valid WAV: {e}"));
                Arc::new(mono(pcm))
            })
            .collect()
    });
    let i = bundled::SOUNDS.iter().position(|(s, _)| *s == id)?;
    Some(clips[i].clone())
}

/// What an event plays: the user's selection, or what is in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The event's default (`default`).
    Default,
    /// A bundled sound (`library:<name>`).
    Library(String),
    /// The user's own file in the folder (`custom`).
    Custom,
    /// Silence (`none`).
    Silent,
}

impl Source {
    /// From the protocol form: `default`, `library:<name>`, `custom` or
    /// `none`. The library name is not checked here.
    pub fn parse(s: &str) -> Option<Source> {
        match s {
            "default" => Some(Source::Default),
            "custom" => Some(Source::Custom),
            "none" => Some(Source::Silent),
            _ => s
                .strip_prefix("library:")
                .filter(|id| !id.is_empty())
                .map(|id| Source::Library(id.to_string())),
        }
    }

    /// The protocol form.
    pub fn as_string(&self) -> String {
        match self {
            Source::Default => "default".into(),
            Source::Library(id) => format!("library:{id}"),
            Source::Custom => "custom".into(),
            Source::Silent => "none".into(),
        }
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

/// The earcons in force: the bundled library, the user's selections and
/// the custom files of a folder (module docs). Shared by every player.
pub struct Library {
    dir: Option<PathBuf>,
    log: Option<Log>,
    slots: Mutex<HashMap<Earcon, Slot>>,
    selection: Mutex<HashMap<Earcon, Source>>,
}

impl Library {
    /// The bundled sounds only, no folder.
    pub fn bundled() -> Library {
        Library {
            dir: None,
            log: None,
            slots: Mutex::new(HashMap::new()),
            selection: Mutex::new(HashMap::new()),
        }
    }

    /// The bundled sounds and `dir`'s `<kind>.wav` files. Every kind is
    /// looked at now, so a bad file is logged at startup.
    pub fn new(dir: PathBuf, log: Option<Log>) -> Library {
        let lib = Library {
            dir: Some(dir),
            log,
            slots: Mutex::new(HashMap::new()),
            selection: Mutex::new(HashMap::new()),
        };
        let _ = lib.custom();
        lib
    }

    /// Start with these selections (persisted ones). One that cannot apply
    /// is kept: `effective` gives the default for it.
    pub fn with_selection(self, selection: impl IntoIterator<Item = (Earcon, Source)>) -> Library {
        self.lock_selection().extend(selection);
        self
    }

    /// The custom earcons folder, if any.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The file that holds `e`'s own sound (whether it exists or not).
    pub fn path(&self, e: Earcon) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join(format!("{}.wav", e.as_str())))
    }

    fn lock_selection(&self) -> MutexGuard<'_, HashMap<Earcon, Source>> {
        self.selection.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The user's selection for `e`, if any.
    pub fn selection(&self, e: Earcon) -> Option<Source> {
        self.lock_selection().get(&e).cloned()
    }

    /// Every selection, in `Earcon::ALL` order (for persisting).
    pub fn selections(&self) -> Vec<(Earcon, Source)> {
        let sel = self.lock_selection();
        Earcon::ALL
            .iter()
            .filter_map(|e| sel.get(e).map(|s| (*e, s.clone())))
            .collect()
    }

    /// What `e` plays now: a library sound, `Custom` or `Silent` (never
    /// `Default`). A selection that cannot apply (a sound no longer
    /// bundled, an own file gone) gives the default.
    pub fn effective(&self, e: Earcon) -> Source {
        let custom_ok = || self.refresh(e).is_some();
        match self.selection(e) {
            Some(Source::Silent) => Source::Silent,
            Some(Source::Library(id)) if sound_clip(&id).is_some() => Source::Library(id),
            Some(Source::Custom) if custom_ok() => Source::Custom,
            None if custom_ok() => Source::Custom,
            _ => e.default_source(),
        }
    }

    /// The clip to play for `e` now (`None`: silent).
    pub fn clip(&self, e: Earcon) -> Option<Arc<PcmChunk>> {
        self.source_clip(Some(e), &self.effective(e)).ok().flatten()
    }

    /// The clip of `source` (for `e` when it is `custom` or `default`):
    /// `Ok(None)` for silence, `Err` when there is nothing to play.
    pub fn source_clip(
        &self,
        e: Option<Earcon>,
        source: &Source,
    ) -> Result<Option<Arc<PcmChunk>>, String> {
        match source {
            Source::Silent => Ok(None),
            Source::Library(id) => sound_clip(id)
                .map(Some)
                .ok_or_else(|| format!("there is no bundled sound '{id}'")),
            Source::Default => {
                let e = e.ok_or("'default' needs the earcon kind")?;
                self.source_clip(Some(e), &e.default_source())
            }
            Source::Custom => {
                let e = e.ok_or("'custom' needs the earcon kind")?;
                self.refresh(e)
                    .map(Some)
                    .ok_or_else(|| format!("there is no usable own sound for {}", e.as_str()))
            }
        }
    }

    /// Select what `e` plays. A library sound must exist and `custom`
    /// needs a usable own file.
    pub fn select(&self, e: Earcon, source: Source) -> Result<(), String> {
        self.source_clip(Some(e), &source)?;
        self.lock_selection().insert(e, source);
        Ok(())
    }

    /// Save `bytes` (a WAV) as `e`'s own sound and select it. It is checked
    /// as a dropped-in file is (`check`) and must be at most
    /// `MAX_UPLOAD_BYTES`; it is stored as mono 16-bit.
    pub fn save_custom(&self, e: Earcon, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_UPLOAD_BYTES {
            return Err(format!(
                "the sound is bigger than {} KB",
                MAX_UPLOAD_BYTES / 1024
            ));
        }
        let pcm = check(bytes)?;
        let path = self
            .path(e)
            .ok_or("this runtime has no folder for your own sounds")?;
        let write = || -> std::io::Result<()> {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            let tmp = path.with_extension("wav.tmp");
            std::fs::write(&tmp, wav::encode(&pcm))?;
            std::fs::rename(&tmp, &path).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
        };
        write().map_err(|err| format!("cannot save {}: {err}", path.display()))?;
        if self.refresh(e).is_none() {
            return Err(format!("{} could not be read back", path.display()));
        }
        self.lock_selection().insert(e, Source::Custom);
        Ok(())
    }

    /// Delete `e`'s own sound. A selection of it is dropped (the default
    /// plays). Returns whether there was a file.
    pub fn delete_custom(&self, e: Earcon) -> Result<bool, String> {
        let Some(path) = self.path(e) else {
            return Ok(false);
        };
        let existed = match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(format!("cannot delete {}: {err}", path.display())),
        };
        {
            let mut sel = self.lock_selection();
            if sel.get(&e) == Some(&Source::Custom) {
                sel.remove(&e);
            }
        }
        let _ = self.refresh(e);
        Ok(existed)
    }

    /// The kinds a usable custom file exists for now, in `Earcon::ALL`
    /// order (selected or not).
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
                    "earcons: {} is gone; {} plays its default",
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
                        "earcons: {} is the own sound of {}",
                        path.display(),
                        e.as_str()
                    ));
                    Some(Arc::new(c))
                }
                Err(why) => {
                    self.note(&format!(
                        "earcons: {}: {why}; {} plays its default",
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
    check(&bytes)
}

/// Decode and check a custom earcon: a usable WAV, not empty or silent, at
/// most `MAX_SECONDS`. Mixed down to mono.
pub fn check(bytes: &[u8]) -> Result<PcmChunk, String> {
    let pcm = wav::decode(bytes).map_err(|e| format!("not a usable WAV ({e})"))?;
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
    fn every_bundled_sound_decodes_to_a_short_mono_clip() {
        assert!(!sounds().is_empty());
        for id in sounds() {
            let c = sound_clip(id).unwrap();
            assert_eq!(c.channels, 1, "{id}");
            assert!(c.sample_rate >= 8_000, "{id}");
            let secs = c.samples.len() as f32 / c.sample_rate as f32;
            assert!(secs > 0.01 && secs <= MAX_SECONDS, "{id} lasts {secs} s");
            assert!(c.samples.iter().any(|&s| s != 0), "{id} is silent");
        }
        assert!(sound_clip("no-such-sound").is_none());
    }

    #[test]
    fn the_library_is_the_sounds_folder() {
        // Adding or removing a sound is a file change only (#211).
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("sounds");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")))
            .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
            .collect();
        on_disk.sort();
        assert_eq!(sounds(), on_disk);
        for e in Earcon::ALL {
            if let Some(id) = e.default_sound() {
                assert!(sounds().contains(&id), "{e:?} defaults to {id}");
            }
            let lib = Library::bundled();
            assert_eq!(lib.effective(*e), e.default_source());
            assert_eq!(lib.clip(*e).is_some(), e.default_sound().is_some());
        }
    }

    #[test]
    fn sound_labels_and_sources() {
        assert_eq!(sound_label("reply-done"), "Reply done");
        assert_eq!(sound_label("nav_edge"), "Nav edge");
        for s in ["default", "custom", "none", "library:edge"] {
            assert_eq!(Source::parse(s).unwrap().as_string(), s);
        }
        assert_eq!(Source::parse("library:"), None);
        assert_eq!(Source::parse("bundled"), None);
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
            let c = lib.clip(e).unwrap();
            assert_eq!(lib.effective(e), Source::Custom);
            assert_eq!((c.channels, c.sample_rate), (1, rate), "{e:?}");
            assert_eq!(c.samples.len(), rate as usize / 10, "{e:?} frames");
            let peak = c.samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(
                (16_000..=16_500).contains(&peak),
                "{e:?} {bits} bit: {peak}"
            );
        }
        // The kinds without a file keep their default.
        assert_eq!(
            lib.effective(Earcon::NavEdge),
            Earcon::NavEdge.default_source()
        );
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
            lib.effective(Earcon::SessionChange),
            Earcon::SessionChange.default_source()
        );
        {
            let l = lines.lock().unwrap();
            assert_eq!(l.len(), 1, "{l:?}");
            assert!(
                l[0].contains("session_change.wav") && l[0].contains("default"),
                "{l:?}"
            );
        }
        // Replaced with a good file: used at the next play.
        std::fs::write(&path, fixture(1, 16, 1, 16_000, 1600)).unwrap();
        assert_eq!(lib.clip(Earcon::SessionChange).unwrap().sample_rate, 16_000);
        assert_eq!(lib.custom(), [Earcon::SessionChange]);
        // Removed: the default again.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            lib.effective(Earcon::SessionChange),
            Earcon::SessionChange.default_source()
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
        assert!(lib
            .save_custom(Earcon::Choice, &fixture(1, 16, 1, 8000, 800))
            .is_err());
        assert_eq!(lib.delete_custom(Earcon::Choice), Ok(false));
    }

    /// A library sound that is not `e`'s default.
    fn other_sound(e: Earcon) -> &'static str {
        sounds()
            .into_iter()
            .find(|id| Some(*id) != e.default_sound())
            .expect("the library has two sounds")
    }

    #[test]
    fn a_selection_decides_what_plays() {
        let dir = tmp();
        let lib = Library::new(dir.to_path_buf(), None);
        let e = Earcon::TurnDone;
        let other = other_sound(e);
        lib.select(e, Source::Library(other.into())).unwrap();
        assert_eq!(lib.effective(e), Source::Library(other.into()));
        assert_eq!(lib.clip(e), sound_clip(other));
        lib.select(e, Source::Silent).unwrap();
        assert_eq!(lib.effective(e), Source::Silent);
        assert!(lib.clip(e).is_none());
        // Refused: an unknown sound, and an own sound that is not there.
        assert!(lib.select(e, Source::Library("nope".into())).is_err());
        assert!(lib.select(e, Source::Custom).is_err());
        assert_eq!(lib.selection(e), Some(Source::Silent), "unchanged");
        // An own file wins without a selection, not over an explicit one.
        std::fs::write(lib.path(e).unwrap(), fixture(1, 16, 1, 8000, 800)).unwrap();
        assert_eq!(lib.effective(e), Source::Silent);
        lib.select(e, Source::Default).unwrap();
        assert_eq!(lib.effective(e), e.default_source());
        lib.select(e, Source::Custom).unwrap();
        assert_eq!(lib.effective(e), Source::Custom);
        assert_eq!(lib.selections(), [(e, Source::Custom)]);
        let fresh = Library::new(dir.to_path_buf(), None);
        assert_eq!(fresh.effective(e), Source::Custom, "a file alone is custom");
    }

    #[test]
    fn a_persisted_selection_that_cannot_apply_gives_the_default() {
        let dir = tmp();
        let lib = Library::new(dir.to_path_buf(), None).with_selection([
            (Earcon::Nav, Source::Library("gone".into())),
            (Earcon::Choice, Source::Custom),
            (Earcon::Error, Source::Silent),
        ]);
        assert_eq!(lib.effective(Earcon::Nav), Earcon::Nav.default_source());
        assert_eq!(
            lib.effective(Earcon::Choice),
            Earcon::Choice.default_source()
        );
        assert_eq!(lib.effective(Earcon::Error), Source::Silent);
        // Kept, so a sound or file coming back applies again.
        assert_eq!(lib.selections().len(), 3);
    }

    #[test]
    fn preview_sources() {
        let lib = Library::bundled();
        let e = Earcon::Nav;
        assert_eq!(
            lib.source_clip(Some(e), &Source::Default).unwrap(),
            e.default_sound().and_then(sound_clip)
        );
        let id = sounds()[0];
        assert_eq!(
            lib.source_clip(None, &Source::Library(id.into())).unwrap(),
            sound_clip(id)
        );
        assert_eq!(lib.source_clip(None, &Source::Silent).unwrap(), None);
        assert!(lib.source_clip(None, &Source::Custom).is_err());
        assert!(lib.source_clip(None, &Source::Default).is_err());
        assert!(lib.source_clip(Some(e), &Source::Custom).is_err());
    }

    #[test]
    fn saving_an_own_sound_checks_it_stores_mono_16_bit_and_selects_it() {
        let dir = tmp();
        let (log, _) = logged();
        let lib = Library::new(dir.to_path_buf(), Some(log));
        let e = Earcon::Permission;
        lib.select(e, Source::Silent).unwrap();
        assert!(lib
            .save_custom(e, b"not a wav")
            .unwrap_err()
            .contains("WAV"));
        let silent = PcmChunk {
            samples: vec![0; 800],
            sample_rate: 8000,
            channels: 1,
        };
        assert!(lib
            .save_custom(e, &wav::encode(&silent))
            .unwrap_err()
            .contains("silent"));
        let big = vec![0u8; MAX_UPLOAD_BYTES + 1];
        assert!(lib.save_custom(e, &big).unwrap_err().contains("bigger"));
        assert!(!lib.path(e).unwrap().exists(), "nothing saved");
        assert_eq!(lib.selection(e), Some(Source::Silent));
        lib.save_custom(e, &fixture(3, 32, 2, 22_050, 2205))
            .unwrap();
        assert_eq!(lib.selection(e), Some(Source::Custom));
        let stored = wav::decode(&std::fs::read(lib.path(e).unwrap()).unwrap()).unwrap();
        assert_eq!((stored.channels, stored.sample_rate), (1, 22_050));
        assert_eq!(lib.clip(e).unwrap().samples.len(), 2205);
        // Deleting it drops the selection: the default plays.
        assert_eq!(lib.delete_custom(e), Ok(true));
        assert_eq!(lib.selection(e), None);
        assert_eq!(lib.effective(e), e.default_source());
        assert_eq!(lib.delete_custom(e), Ok(false));
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
