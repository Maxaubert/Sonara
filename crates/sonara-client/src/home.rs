//! The home folder and the names of the files clients look for in it.
use std::path::{Path, PathBuf};

/// How clients find and authenticate to the running runtime (written by
/// `sonarad`, spec section 3).
pub const RUNTIME_FILE: &str = "runtime.json";

/// The stop sentinel in the home: while it exists nothing starts the
/// runtime (the user shut Sonara down), like the Python plugin's `stopped`
/// file. Messages still reach a runtime that is running.
pub const STOPPED: &str = "stopped";

/// `%LOCALAPPDATA%\Sonara`, if `LOCALAPPDATA` is set and not empty.
pub fn default_home(localappdata: Option<&str>) -> Option<PathBuf> {
    localappdata
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join("Sonara"))
}

/// The home from the two environment values: `SONARA_HOME` when set and
/// not empty, else `%LOCALAPPDATA%\Sonara`.
pub fn home_from(sonara_home: Option<&str>, localappdata: Option<&str>) -> Option<PathBuf> {
    match sonara_home.filter(|h| !h.is_empty()) {
        Some(h) => Some(PathBuf::from(h)),
        None => default_home(localappdata),
    }
}

/// The runtime's home, read through `env` (`home_from`, as `sonarad`).
pub fn home(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    home_from(
        env("SONARA_HOME").as_deref(),
        env("LOCALAPPDATA").as_deref(),
    )
}

/// Whether the user shut Sonara down in `home` (`STOPPED`).
pub fn stopped(home: &Path) -> bool {
    home.join(STOPPED).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonara_home_beats_localappdata() {
        let env = |k: &str| match k {
            "LOCALAPPDATA" => Some(r"C:\Users\x\AppData\Local".to_string()),
            _ => None,
        };
        assert_eq!(
            home(&env),
            Some(PathBuf::from(r"C:\Users\x\AppData\Local\Sonara"))
        );
        let env2 = |k: &str| match k {
            "SONARA_HOME" => Some(r"D:\h".to_string()),
            "LOCALAPPDATA" => Some(r"C:\l".to_string()),
            _ => None,
        };
        assert_eq!(home(&env2), Some(PathBuf::from(r"D:\h")));
        assert_eq!(home(&|_| None), None);
        assert_eq!(
            home_from(Some(""), Some(r"C:\l")),
            Some(Path::new(r"C:\l").join("Sonara"))
        );
        assert_eq!(home_from(None, Some("")), None);
    }

    #[test]
    fn the_stop_sentinel() {
        let dir = std::env::temp_dir().join(format!("sonara-client-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!stopped(&dir));
        std::fs::write(dir.join(STOPPED), b"").unwrap();
        assert!(stopped(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
