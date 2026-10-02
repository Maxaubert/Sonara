//! The model manager: the Kokoro files in `<home>\models\kokoro\v1.0\`,
//! downloaded on first use from pinned URLs with pinned SHA-256 values.
//!
//! - A download goes to `<file>.part` and resumes with an HTTP range
//!   request; a finished file is hashed and renamed into place atomically,
//!   so a file under its final name is always complete.
//! - A host may pre-seed the folder: a file with the pinned size and hash
//!   is used as it is (hashed once, then remembered in `verified.json` by
//!   size and modification time).
//! - A failure (offline, a hash mismatch) puts the manager in `Failed`
//!   until `retry_at`; the delay doubles per failure up to `Backoff::max`,
//!   so callers that ask again on every sentence cause no retry storm.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Where the pinned files come from: the release assets kokoro-onnx uses
/// (Kokoro-82M v1.0 exported to ONNX, Apache-2.0 weights).
pub const BASE_URL: &str =
    "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0";
/// The model folder under `<home>\models\`.
pub const MODEL_SUBDIR: &str = "kokoro/v1.0";
pub const MODEL_FILE: &str = "kokoro-v1.0.onnx";
pub const VOICES_FILE: &str = "voices-v1.0.bin";
const MARKER: &str = "verified.json";

/// One file to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    pub name: String,
    pub url: String,
    /// Lower-case hex.
    pub sha256: String,
    pub size: u64,
}

/// The pinned Kokoro v1.0 files (fp32 model, see the M4 notes in
/// `docs/plans/2026-10-02-m0-engine-spike.md`) under `base` (`BASE_URL`, or
/// a mirror or test server with the same file names).
pub fn pinned_files(base: &str) -> Vec<ModelFile> {
    let base = base.trim_end_matches('/');
    [
        (
            MODEL_FILE,
            "7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5",
            325_532_387,
        ),
        (
            VOICES_FILE,
            "bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d",
            28_214_398,
        ),
    ]
    .into_iter()
    .map(|(name, sha, size)| ModelFile {
        name: name.to_string(),
        url: format!("{base}/{name}"),
        sha256: sha.to_string(),
        size,
    })
    .collect()
}

/// How long to wait after a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub first: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    /// 30 s after the first failure, doubling, at most 30 minutes (the
    /// Python reader waited 30 minutes after any failure).
    fn default() -> Self {
        Backoff {
            first: Duration::from_secs(30),
            max: Duration::from_secs(30 * 60),
        }
    }
}

impl Backoff {
    /// The wait after `failures` failures in a row (at least one).
    pub fn delay(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(20);
        self.first.saturating_mul(1u32 << doublings).min(self.max)
    }
}

/// What the manager is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Nothing checked yet in this run.
    Idle,
    /// Hashing files already on disk.
    Verifying,
    /// Bytes done and expected over all files.
    Downloading {
        done: u64,
        total: u64,
    },
    Ready,
    Failed {
        reason: String,
        retry_at: Instant,
    },
}

struct State {
    phase: Phase,
    failures: u32,
    running: bool,
}

pub struct Manager {
    dir: PathBuf,
    files: Vec<ModelFile>,
    backoff: Backoff,
    state: Mutex<State>,
    agent: ureq::Agent,
}

/// Entries of `verified.json`: a file that kept its size and modification
/// time since it was hashed is not hashed again.
type Verified = BTreeMap<String, (u64, u128, String)>;

fn modified_ns(meta: &fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        // A whole file (325 MB) on a slow line; a stall ends the attempt.
        .timeout_recv_body(Some(Duration::from_secs(60 * 60)))
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::Rustls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .user_agent(concat!("sonara/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

impl Manager {
    pub fn new(dir: PathBuf, files: Vec<ModelFile>, backoff: Backoff) -> Manager {
        Manager {
            dir,
            files,
            backoff,
            state: Mutex::new(State {
                phase: Phase::Idle,
                failures: 0,
                running: false,
            }),
            agent: agent(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn phase(&self) -> Phase {
        self.lock().phase.clone()
    }

    pub fn is_running(&self) -> bool {
        self.lock().running
    }

    fn total(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    fn read_verified(&self) -> Verified {
        fs::read_to_string(self.path(MARKER))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_verified(&self, v: &Verified) {
        if let Ok(text) = serde_json::to_string_pretty(v) {
            let tmp = self.path(&format!("{MARKER}.tmp"));
            if fs::write(&tmp, text).is_ok() {
                let _ = fs::rename(&tmp, self.path(MARKER));
            }
        }
    }

    /// True when `f` is in place and was verified with its current size
    /// and modification time (no hashing).
    fn verified(&self, f: &ModelFile, v: &Verified) -> bool {
        let Ok(meta) = fs::metadata(self.path(&f.name)) else {
            return false;
        };
        meta.len() == f.size
            && v.get(&f.name) == Some(&(f.size, modified_ns(&meta), f.sha256.to_ascii_lowercase()))
    }

    /// Every file is in place and verified. Cheap: no hashing.
    pub fn present(&self) -> bool {
        let v = self.read_verified();
        self.files.iter().all(|f| self.verified(f, &v))
    }

    /// Every file is on disk with its pinned size (verified or not): a
    /// fetch only hashes, no download. Cheap.
    pub fn on_disk(&self) -> bool {
        self.files
            .iter()
            .all(|f| fs::metadata(self.path(&f.name)).map(|m| m.len()).ok() == Some(f.size))
    }

    /// Claim the right to fetch now: false while a fetch runs or before
    /// the retry time of the last failure. A `true` must be followed by
    /// `fetch`.
    pub fn begin(&self, now: Instant) -> bool {
        let mut s = self.lock();
        if s.running {
            return false;
        }
        if let Phase::Failed { retry_at, .. } = &s.phase {
            if now < *retry_at {
                return false;
            }
        }
        s.running = true;
        true
    }

    fn set_phase(&self, phase: Phase) {
        self.lock().phase = phase;
    }

    /// Verify what is on disk and download what is missing (blocking).
    /// Call after `begin` returned true. On failure the manager waits out
    /// its backoff before the next `begin` succeeds.
    pub fn fetch(&self) -> Result<(), String> {
        let result = self.fetch_all();
        self.finish(result)
    }

    /// End a claimed fetch that could not run (`begin` returned true).
    pub fn fetch_failed(&self, reason: String) -> Result<(), String> {
        self.finish(Err(reason))
    }

    fn finish(&self, result: Result<(), String>) -> Result<(), String> {
        let mut s = self.lock();
        s.running = false;
        match &result {
            Ok(()) => {
                s.failures = 0;
                s.phase = Phase::Ready;
            }
            Err(reason) => {
                s.failures += 1;
                s.phase = Phase::Failed {
                    reason: reason.clone(),
                    retry_at: Instant::now() + self.backoff.delay(s.failures),
                };
            }
        }
        result
    }

    fn fetch_all(&self) -> Result<(), String> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        let mut verified = self.read_verified();
        let mut done = 0u64;
        let total = self.total();
        for f in &self.files {
            let path = self.path(&f.name);
            if !self.verified(f, &verified) && path.is_file() {
                self.set_phase(Phase::Verifying);
                let matches = fs::metadata(&path).map(|m| m.len()).ok() == Some(f.size)
                    && hash_file(&path)? == f.sha256.to_ascii_lowercase();
                if matches {
                    self.remember(f, &mut verified)?;
                } else {
                    // Not ours (or damaged): fetch it again.
                    fs::remove_file(&path)
                        .map_err(|e| format!("cannot replace {}: {e}", path.display()))?;
                }
            }
            if !self.verified(f, &verified) {
                self.download(f, done, total)?;
                self.remember(f, &mut verified)?;
            }
            done += f.size;
        }
        Ok(())
    }

    fn remember(&self, f: &ModelFile, verified: &mut Verified) -> Result<(), String> {
        let meta = fs::metadata(self.path(&f.name)).map_err(|e| e.to_string())?;
        verified.insert(
            f.name.clone(),
            (
                meta.len(),
                modified_ns(&meta),
                f.sha256.to_ascii_lowercase(),
            ),
        );
        self.write_verified(verified);
        Ok(())
    }

    /// Fetch one file into `<name>.part` (resuming), check its hash and
    /// rename it into place.
    fn download(&self, f: &ModelFile, before: u64, total: u64) -> Result<(), String> {
        let part = self.path(&format!("{}.part", f.name));
        let mut restarted = false;
        loop {
            let have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if have > f.size {
                let _ = fs::remove_file(&part);
                continue;
            }
            self.set_phase(Phase::Downloading {
                done: before + have,
                total,
            });
            if have < f.size {
                match self.request(f, &part, have, before, total)? {
                    Fetched::Done => {}
                    Fetched::Restart if !restarted => {
                        restarted = true;
                        let _ = fs::remove_file(&part);
                        continue;
                    }
                    Fetched::Restart => {
                        return Err(format!("{}: the server cannot resume", f.name))
                    }
                }
            }
            let got = hash_file(&part)?;
            if got != f.sha256.to_ascii_lowercase() {
                let _ = fs::remove_file(&part);
                return Err(format!(
                    "{}: hash mismatch (got {got}, expected {})",
                    f.name, f.sha256
                ));
            }
            let dest = self.path(&f.name);
            return fs::rename(&part, &dest)
                .map_err(|e| format!("cannot move {} into place: {e}", dest.display()));
        }
    }

    fn request(
        &self,
        f: &ModelFile,
        part: &Path,
        have: u64,
        before: u64,
        total: u64,
    ) -> Result<Fetched, String> {
        let mut req = self.agent.get(&f.url);
        if have > 0 {
            req = req.header("Range", &format!("bytes={have}-"));
        }
        let mut resp = req
            .call()
            .map_err(|e| format!("cannot download {}: {e}", f.name))?;
        let status = resp.status().as_u16();
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(part)
            .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
        let mut at = match status {
            206 => {
                // The range must start where the file ends.
                let start = resp
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("bytes "))
                    .and_then(|v| v.split('-').next())
                    .and_then(|v| v.trim().parse::<u64>().ok());
                if start != Some(have) {
                    return Ok(Fetched::Restart);
                }
                have
            }
            200 => {
                file.set_len(0).map_err(|e| e.to_string())?;
                0
            }
            416 => return Ok(Fetched::Restart),
            other => return Err(format!("cannot download {}: HTTP {other}", f.name)),
        };
        file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        let mut body = resp.body_mut().as_reader();
        let mut buf = vec![0u8; 1 << 20];
        let mut last = Instant::now();
        loop {
            let n = body
                .read(&mut buf)
                .map_err(|e| format!("download of {} broke off: {e}", f.name))?;
            if n == 0 {
                break;
            }
            if at + n as u64 > f.size {
                return Err(format!("{}: the server sent more than expected", f.name));
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
            at += n as u64;
            if last.elapsed() >= Duration::from_millis(200) {
                last = Instant::now();
                self.set_phase(Phase::Downloading {
                    done: before + at,
                    total,
                });
            }
        }
        file.sync_all().map_err(|e| e.to_string())?;
        if at < f.size {
            return Err(format!(
                "download of {} ended early ({at} of {} bytes); it resumes later",
                f.name, f.size
            ));
        }
        Ok(Fetched::Done)
    }
}

enum Fetched {
    Done,
    /// Start over without a range (the server ignored or refused it).
    Restart,
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let b = Backoff::default();
        assert_eq!(b.delay(1), Duration::from_secs(30));
        assert_eq!(b.delay(2), Duration::from_secs(60));
        assert_eq!(b.delay(4), Duration::from_secs(240));
        assert_eq!(b.delay(7), Duration::from_secs(30 * 60));
        assert_eq!(b.delay(1_000), Duration::from_secs(30 * 60));
    }

    #[test]
    fn pinned_files_use_the_base_url() {
        let f = pinned_files("http://127.0.0.1:9/m/");
        assert_eq!(f[0].url, "http://127.0.0.1:9/m/kokoro-v1.0.onnx");
        assert_eq!(f[1].name, VOICES_FILE);
        assert_eq!(pinned_files(BASE_URL)[0].size, 325_532_387);
    }
}
