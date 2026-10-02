//! The home folder (spec section 3): `--home`, else `SONARA_HOME`, else
//! `%LOCALAPPDATA%\Sonara`. Every path under it is built here.
use std::path::{Path, PathBuf};

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
    /// (the migration, settings that could not be applied).
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("logs").join("sonarad.log")
    }

    /// Print a note on stderr and append it, timestamped, to the log file
    /// (best effort).
    pub fn log(&self, line: &str) {
        use std::io::Write;
        eprintln!("sonarad: {line}");
        let path = self.log_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let now = crate::runtime_file::rfc3339(std::time::SystemTime::now());
            let _ = writeln!(f, "{now} {line}");
        }
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
}
