//! The home folder (spec section 3): `--home`, else `SONARA_HOME`, else
//! `%LOCALAPPDATA%\Sonara`. Every path under it is built here.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `sonarad.log` stays below this size: a line that would pass it first
/// renames the file to `sonarad.old.log` (replacing the older one), so the
/// two files hold at most about 2 MB (#217).
pub const LOG_CAP: u64 = 1_000_000;

/// Serializes appends and the rotation: several threads log (the reader's
/// watcher, the audio worker, hotkeys, requests).
static LOG_LOCK: Mutex<()> = Mutex::new(());

/// Append `line` and a newline to `path`; when the file would pass `cap`,
/// rename it to `old` (replacing it) first. Best effort.
pub fn append_line(path: &Path, old: &Path, cap: u64, line: &str) {
    use std::io::Write;
    let _guard = LOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len > 0 && len + line.len() as u64 + 1 > cap {
        let _ = std::fs::rename(path, old);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    pub dir: PathBuf,
    /// The home is `%LOCALAPPDATA%\Sonara` (the single-instance mutex is then
    /// per user only; see `instance::mutex_name`).
    pub is_default: bool,
}

/// `%LOCALAPPDATA%\Sonara`, if `LOCALAPPDATA` is set.
pub fn default_dir(localappdata: Option<&str>) -> Option<PathBuf> {
    localappdata
        .filter(|s| !s.is_empty())
        .map(|d| Path::new(d).join("Sonara"))
}

/// Pick the home from the flag and the environment values given.
pub fn choose(
    flag: Option<&Path>,
    sonara_home: Option<&str>,
    localappdata: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(f) = flag {
        return Ok(f.to_path_buf());
    }
    if let Some(h) = sonara_home.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(h));
    }
    default_dir(localappdata).ok_or_else(|| {
        "no home folder: set SONARA_HOME, pass --home, or set LOCALAPPDATA".to_string()
    })
}

/// Comparable form of an existing folder.
fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// A stable text key for a home, for the mutex name: the canonical path,
/// lower-cased (Windows paths compare case-insensitively).
pub fn key(home: &Home) -> String {
    canonical(&home.dir).to_string_lossy().to_lowercase()
}

/// Resolve and create the home.
pub fn resolve(flag: Option<&Path>) -> Result<Home, String> {
    let sonara_home = std::env::var("SONARA_HOME").ok();
    let local = std::env::var("LOCALAPPDATA").ok();
    let dir = choose(flag, sonara_home.as_deref(), local.as_deref())?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create the home folder {}: {e}", dir.display()))?;
    let is_default = default_dir(local.as_deref())
        .map(|d| d.is_dir() && canonical(&d) == canonical(&dir))
        .unwrap_or(false);
    Ok(Home { dir, is_default })
}

impl Home {
    pub fn runtime_json(&self) -> PathBuf {
        self.dir.join("runtime.json")
    }

    /// `logs\sonarad.log`: notes worth keeping after the process is gone
    /// (the migration, settings that could not be applied) and the
    /// activity lines (reading, other apps paused or ducked, hotkeys).
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("logs").join("sonarad.log")
    }

    /// `logs\sonarad.old.log`: the log before the last rotation.
    pub fn old_log_path(&self) -> PathBuf {
        self.dir.join("logs").join("sonarad.old.log")
    }

    /// Print a note on stderr and append it, with a UTC timestamp, to the
    /// log file (best effort; rotated at `LOG_CAP`).
    pub fn log(&self, line: &str) {
        eprintln!("sonarad: {line}");
        let now = crate::runtime_file::rfc3339(std::time::SystemTime::now());
        append_line(
            &self.log_path(),
            &self.old_log_path(),
            LOG_CAP,
            &format!("{now} {line}"),
        );
    }

    /// `earcons\`: `<kind>.wav` files that replace the bundled earcons
    /// (`sonara_agent::earcon::Library`).
    pub fn earcons(&self) -> PathBuf {
        self.dir.join("earcons")
    }

    /// `models\<engine>\<version>\` (spec section 3).
    pub fn models(&self) -> PathBuf {
        self.dir.join("models")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_beats_the_environment_which_beats_localappdata() {
        let flag = Path::new(r"C:\flag");
        assert_eq!(
            choose(Some(flag), Some(r"C:\env"), Some(r"C:\lad")).unwrap(),
            PathBuf::from(r"C:\flag")
        );
        assert_eq!(
            choose(None, Some(r"C:\env"), Some(r"C:\lad")).unwrap(),
            PathBuf::from(r"C:\env")
        );
        assert_eq!(
            choose(None, Some(""), Some(r"C:\lad")).unwrap(),
            Path::new(r"C:\lad").join("Sonara")
        );
        assert!(choose(None, None, None).is_err());
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sonarad-home-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_full_log_rotates_to_the_old_file_once() {
        let dir = tmp("rotate");
        let (log, old) = (
            dir.join("logs").join("a.log"),
            dir.join("logs").join("a.old.log"),
        );
        // 10 bytes per line ("line nnnn" and the newline), cap 35: three
        // lines fit, the fourth starts a new file.
        for i in 0..4 {
            append_line(&log, &old, 35, &format!("line {i:04}"));
        }
        assert_eq!(
            std::fs::read_to_string(&old).unwrap(),
            "line 0000\nline 0001\nline 0002\n"
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "line 0003\n");
        // The next rotation replaces the old file.
        for i in 4..8 {
            append_line(&log, &old, 35, &format!("line {i:04}"));
        }
        assert_eq!(
            std::fs::read_to_string(&old).unwrap(),
            "line 0003\nline 0004\nline 0005\n"
        );
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "line 0006\nline 0007\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_writers_lose_no_line_across_a_rotation() {
        let dir = tmp("threads");
        let (log, old) = (
            dir.join("logs").join("b.log"),
            dir.join("logs").join("b.old.log"),
        );
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let (log, old) = (log.clone(), old.clone());
                std::thread::spawn(move || {
                    for i in 0..50 {
                        append_line(&log, &old, 4_000, &format!("t{t} line {i:03}"));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        // 200 lines of 12 bytes = 2400 bytes: under the cap, no rotation.
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 200);
        assert!(text.lines().all(|l| l.len() == 11), "no torn line");
        // Past the cap: the two files together hold every line.
        for i in 0..200 {
            append_line(&log, &old, 4_000, &format!("x{i:03} line 0"));
        }
        let both = std::fs::read_to_string(&old).unwrap() + &std::fs::read_to_string(&log).unwrap();
        assert!(std::fs::metadata(&log).unwrap().len() <= 4_000);
        assert!(both.lines().any(|l| l == "x199 line 0"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
