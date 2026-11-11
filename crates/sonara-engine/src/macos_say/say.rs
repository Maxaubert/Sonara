//! The process side of the macOS engine: one `say` per synthesis, writing a
//! WAV to a temp file that is read and removed.
use super::{
    choose_voice, classify_failure, guard_text, no_voices, parse_voices, say_rate, VoiceInfo,
    DATA_FORMAT, ID, SAY,
};
use crate::{wav, Engine, EngineId, Error, LicenseClass, PcmStream, Result, Voice};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Short text `warm` synthesizes to prove a voice speaks.
const PROBE: &str = "Sonara.";
/// How often a synthesis checks whether its `say` ended.
const POLL: Duration = Duration::from_millis(5);

/// macOS speech through `/usr/bin/say`. Syntheses run concurrently (the
/// reader prefetches one chunk ahead), each in a `say` of its own.
pub struct MacSay {
    program: PathBuf,
    /// Bumped by `cancel`; a synthesis that sees a newer epoch is abandoned.
    epoch: AtomicU64,
    next: AtomicU64,
    /// The `say` processes in flight, killed by `cancel` and on drop.
    running: Mutex<HashMap<u64, Child>>,
    /// The voice list, listed once (`refresh_voices` lists it again).
    voices: Mutex<Option<Vec<VoiceInfo>>>,
}

impl Default for MacSay {
    fn default() -> Self {
        Self::new()
    }
}

/// Removes the temp WAV however the synthesis ends.
struct TempWav(PathBuf);

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl MacSay {
    pub fn new() -> Self {
        Self::with_program(SAY)
    }

    /// The engine on another `say`-compatible program (tests).
    pub fn with_program(program: impl AsRef<Path>) -> Self {
        MacSay {
            program: program.as_ref().to_path_buf(),
            epoch: AtomicU64::new(0),
            next: AtomicU64::new(0),
            running: Mutex::new(HashMap::new()),
            voices: Mutex::new(None),
        }
    }

    fn cannot_run(&self, e: std::io::Error) -> Error {
        Error::Engine(format!(
            "macOS speech is unavailable: cannot run {}: {e}",
            self.program.display()
        ))
    }

    /// The voices `say -v '?'` lists; an empty list is not cached.
    fn voice_infos(&self) -> Result<Vec<VoiceInfo>> {
        if let Some(v) = lock(&self.voices).as_ref() {
            return Ok(v.clone());
        }
        let out = Command::new(&self.program)
            .args(["-v", "?"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| self.cannot_run(e))?;
        if !out.status.success() {
            return Err(classify_failure(&out.status.to_string(), ""));
        }
        let voices = parse_voices(&String::from_utf8_lossy(&out.stdout));
        if !voices.is_empty() {
            *lock(&self.voices) = Some(voices.clone());
        }
        Ok(voices)
    }

    /// Synthesize to the WAV bytes `say` writes.
    fn synthesize_wav(&self, text: &str, voice: &str, rate: u32) -> Result<Vec<u8>> {
        let epoch = self.epoch.load(Ordering::SeqCst);
        let voices = self.voice_infos()?;
        let chosen = choose_voice(&voices, voice)?;
        let key = self.next.fetch_add(1, Ordering::SeqCst);
        let out = TempWav(
            std::env::temp_dir().join(format!("sonara-say-{}-{key}.wav", std::process::id())),
        );
        let mut cmd = Command::new(&self.program);
        if let Some(v) = chosen {
            cmd.arg("-v").arg(&v.name);
        }
        cmd.arg("-r")
            .arg(say_rate(rate).to_string())
            .arg("-o")
            .arg(&out.0)
            .arg(format!("--data-format={DATA_FORMAT}"))
            // The text on stdin: never parsed as an option, no length limit.
            .args(["-f", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| self.cannot_run(e))?;
        let stdin = child.stdin.take();
        let mut stderr = child.stderr.take();
        lock(&self.running).insert(key, child);
        // A cancel since this synthesis began ends it too.
        if self.epoch.load(Ordering::SeqCst) != epoch {
            self.kill(key);
            return Err(Error::Cancelled);
        }
        if let Some(mut stdin) = stdin {
            // A failed write (a killed `say`) shows in its exit status.
            let _ = stdin.write_all(guard_text(text).as_bytes());
        }
        let status = loop {
            let done = {
                let mut running = lock(&self.running);
                match running.get_mut(&key).map(Child::try_wait) {
                    Some(Ok(Some(status))) => {
                        running.remove(&key);
                        Some(Ok(status))
                    }
                    Some(Ok(None)) => None,
                    Some(Err(e)) => {
                        if let Some(mut c) = running.remove(&key) {
                            let _ = c.kill();
                            let _ = c.wait();
                        }
                        Some(Err(e))
                    }
                    None => Some(Err(std::io::Error::other("lost track of say"))),
                }
            };
            match done {
                Some(r) => break r,
                None => std::thread::sleep(POLL),
            }
        };
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return Err(Error::Cancelled);
        }
        let status = status.map_err(|e| Error::Engine(format!("macOS speech failed: {e}")))?;
        if !status.success() {
            let mut msg = String::new();
            if let Some(e) = stderr.as_mut() {
                let _ = e.read_to_string(&mut msg);
            }
            return Err(classify_failure(&status.to_string(), &msg));
        }
        std::fs::read(&out.0).map_err(|e| {
            Error::Engine(format!(
                "macOS speech wrote no audio ({}): {e}",
                out.0.display()
            ))
        })
    }

    /// Kill and reap one `say`.
    fn kill(&self, key: u64) {
        if let Some(mut c) = lock(&self.running).remove(&key) {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for MacSay {
    fn drop(&mut self) {
        for (_, mut c) in lock(&self.running).drain() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Engine for MacSay {
    fn id(&self) -> EngineId {
        ID
    }

    fn license_class(&self) -> LicenseClass {
        LicenseClass::Os
    }

    /// The listed voices; an empty list when `say` cannot list them.
    fn voices(&self) -> Vec<Voice> {
        self.voice_infos()
            .unwrap_or_default()
            .into_iter()
            .map(|i| Voice {
                id: i.name.clone(),
                name: i.name,
                language: i.language,
                engine: ID,
                license_class: LicenseClass::Os,
                installed: true,
            })
            .collect()
    }

    /// List the voices and synthesize a short probe.
    fn warm(&self) -> Result<()> {
        if self.voice_infos()?.is_empty() {
            return Err(no_voices());
        }
        self.synthesize_wav(PROBE, "", 200).map(|_| ())
    }

    fn synthesize(&self, text: &str, voice: &str, rate: u32) -> Result<PcmStream> {
        let bytes = self.synthesize_wav(text, voice, rate)?;
        let pcm = wav::decode(&bytes)?;
        Ok(Box::new(std::iter::once(Ok(pcm))))
    }

    /// Abandon every synthesis in flight: its `say` is killed (the waiting
    /// synthesis reaps it and returns `Error::Cancelled`).
    fn cancel(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        for c in lock(&self.running).values_mut() {
            let _ = c.kill();
        }
    }

    fn refresh_voices(&self) -> Result<Vec<Voice>> {
        *lock(&self.voices) = None;
        Ok(self.voices())
    }
}
