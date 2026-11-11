//! The home folder (spec section 3): `--home`, else `SONARA_HOME`, else
//! `%LOCALAPPDATA%\Sonara` on Windows and
//! `~/Library/Application Support/Sonara` on macOS. Every path under it is
//! built here. The
//! environment half and the names clients look for (`runtime.json`) come
//! from `sonara_client`, so the hook, the CLI and the runtime agree (#255).
use std::path::{Path, PathBuf};

/// The runtime's stream in the log folder (`sonarad.log`).
pub const LOG_STREAM: &str = "sonarad";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    pub dir: PathBuf,
    /// The home is the default one (`sonara_client::default_home`; the
    /// single-instance mutex is then per user only, see
    /// `instance::mutex_name`).
    pub is_default: bool,
}

/// The default home from the value of `sonara_client::BASE_VAR`
/// (`LOCALAPPDATA` on Windows, `HOME` on macOS).
pub fn default_dir(base: Option<&str>) -> Option<PathBuf> {
    sonara_client::default_home(base)
}

/// Pick the home from the flag and the environment values given.
pub fn choose(
    flag: Option<&Path>,
    sonara_home: Option<&str>,
    base: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(f) = flag {
        return Ok(f.to_path_buf());
    }
    sonara_client::home_from(sonara_home, base).ok_or_else(|| {
        format!(
            "no home folder: set SONARA_HOME, pass --home, or set {}",
            sonara_client::BASE_VAR
        )
    })
}

/// Create the home and its parents. On unix the folders created are
/// user-only (0700): the home holds the token's file and the logs.
pub fn create(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)?;
    #[cfg(unix)]
    user_only(dir)?;
    Ok(())
}

/// A home made earlier (or by someone else) must be this user's own folder,
/// not a link, and is narrowed to 0700: the token file lives in it.
#[cfg(unix)]
fn user_only(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() || meta.uid() != uid {
        return Err(std::io::Error::other(
            "it is not a folder owned by this user",
        ));
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
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
    let base = std::env::var(sonara_client::BASE_VAR).ok();
    let dir = choose(flag, sonara_home.as_deref(), base.as_deref())?;
    create(&dir).map_err(|e| format!("cannot create the home folder {}: {e}", dir.display()))?;
    let is_default = default_dir(base.as_deref())
        .map(|d| d.is_dir() && canonical(&d) == canonical(&dir))
        .unwrap_or(false);
    Ok(Home { dir, is_default })
}

impl Home {
    pub fn runtime_json(&self) -> PathBuf {
        self.dir.join(sonara_client::RUNTIME_FILE)
    }

    /// `logs/`: every log file of the home (`sonara_log`: rotating
    /// segments, 10 MB for the whole folder, oldest first out).
    pub fn logs(&self) -> PathBuf {
        self.dir.join("logs")
    }

    /// The log folder with its limits.
    pub fn log_dir(&self) -> sonara_log::LogDir {
        sonara_log::LogDir::new(self.logs())
    }

    /// `logs/sonarad.log`: notes worth keeping after the process is gone
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

    /// `earcons/`: `<kind>.wav` files that replace the bundled earcons
    /// (`sonara_agent::earcon::Library`).
    pub fn earcons(&self) -> PathBuf {
        self.dir.join("earcons")
    }

    /// `models/<engine>/<version>/` (spec section 3).
    pub fn models(&self) -> PathBuf {
        self.dir.join("models")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_beats_the_environment_which_beats_the_default() {
        let flag = Path::new("flag");
        assert_eq!(
            choose(Some(flag), Some("env"), Some("base")).unwrap(),
            PathBuf::from("flag")
        );
        assert_eq!(
            choose(None, Some("env"), Some("base")).unwrap(),
            PathBuf::from("env")
        );
        assert_eq!(
            choose(None, Some(""), Some("base")).unwrap(),
            sonara_client::default_home(Some("base")).unwrap()
        );
        assert!(choose(None, None, None).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn the_default_home_is_under_localappdata() {
        assert_eq!(
            default_dir(Some(r"C:\lad")),
            Some(Path::new(r"C:\lad").join("Sonara"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_default_home_is_in_application_support() {
        assert_eq!(
            default_dir(Some("/Users/x")),
            Some(PathBuf::from("/Users/x/Library/Application Support/Sonara"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_home_is_created_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let top = std::env::temp_dir().join(format!("sonarad-home-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        let dir = top.join("Application Support").join("Sonara");
        create(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
        let _ = std::fs::remove_dir_all(&top);
    }

    #[cfg(unix)]
    #[test]
    fn an_open_home_made_earlier_is_narrowed_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sonarad-home-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        create(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_home_that_is_a_link_is_refused() {
        let top = std::env::temp_dir().join(format!("sonarad-home-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        std::fs::create_dir_all(top.join("real")).unwrap();
        std::os::unix::fs::symlink(top.join("real"), top.join("home")).unwrap();
        assert!(create(&top.join("home")).is_err());
        let _ = std::fs::remove_dir_all(&top);
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
