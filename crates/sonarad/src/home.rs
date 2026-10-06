//! The home folder (spec section 3): `--home`, else `SONARA_HOME`, else
//! `%LOCALAPPDATA%\Sonara`. Every path under it is built here. The
//! environment half and the names clients look for (`runtime.json`) come
//! from `sonara_client`, so the hook, the CLI and the runtime agree (#255).
use std::path::{Path, PathBuf};

/// The runtime's stream in the log folder (`sonarad.log`).
pub const LOG_STREAM: &str = "sonarad";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    pub dir: PathBuf,
    /// The home is `%LOCALAPPDATA%\Sonara` (the single-instance mutex is then
    /// per user only; see `instance::mutex_name`).
    pub is_default: bool,
}

/// `%LOCALAPPDATA%\Sonara`, if `LOCALAPPDATA` is set.
pub fn default_dir(localappdata: Option<&str>) -> Option<PathBuf> {
    sonara_client::default_home(localappdata)
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
    sonara_client::home_from(sonara_home, localappdata).ok_or_else(|| {
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
        self.dir.join(sonara_client::RUNTIME_FILE)
    }

    /// `logs\`: every log file of the home (`sonara_log`: rotating
    /// segments, 10 MB for the whole folder, oldest first out).
    pub fn logs(&self) -> PathBuf {
        self.dir.join("logs")
    }

    /// The log folder with its limits.
    pub fn log_dir(&self) -> sonara_log::LogDir {
        sonara_log::LogDir::new(self.logs())
    }

    /// `logs\sonarad.log`: notes worth keeping after the process is gone
    /// (the migration, settings that could not be applied), the activity
    /// lines (reading, other apps paused or ducked, hotkeys) and the
    /// troubleshooting lines (#219: what was read, what came in, what the
    /// agent decided). Older lines are in `sonarad.<n>.log`.
    pub fn log_path(&self) -> PathBuf {
        self.log_dir().path(LOG_STREAM)
    }

    /// Print a note on stderr and append it, with a UTC timestamp, to the
    /// log (best effort: a line the folder cannot take is dropped).
    pub fn log(&self, line: &str) {
        eprintln!("sonarad: {line}");
        let _ = self.log_dir().log(LOG_STREAM, line);
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

    #[test]
    fn notes_go_to_the_sonarad_stream_of_the_log_folder() {
        let dir = std::env::temp_dir().join(format!("sonarad-home-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home {
            dir: dir.clone(),
            is_default: false,
        };
        home.log("first note");
        assert_eq!(home.log_path(), dir.join("logs").join("sonarad.log"));
        let text = std::fs::read_to_string(home.log_path()).unwrap();
        let (stamp, rest) = text.trim_end().split_once(' ').unwrap();
        assert_eq!(rest, "first note");
        assert!(stamp.ends_with('Z') && stamp.contains('T'), "{stamp}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
