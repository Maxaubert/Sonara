//! The home folder and the names of the files clients look for in it.
use std::path::{Path, PathBuf};

/// How clients find and authenticate to the running runtime (written by
/// `sonarad`, spec section 3).
pub const RUNTIME_FILE: &str = "runtime.json";

/// The stop sentinel in the home: while it exists nothing starts the
/// runtime (the user shut Sonara down), like the Python plugin's `stopped`
/// file. Messages still reach a runtime that is running.
pub const STOPPED: &str = "stopped";

/// The environment variable the default home is built on: `LOCALAPPDATA`
/// on Windows, `HOME` on macOS.
#[cfg(windows)]
pub const BASE_VAR: &str = "LOCALAPPDATA";
/// The environment variable the default home is built on: `LOCALAPPDATA`
/// on Windows, `HOME` on macOS.
#[cfg(not(windows))]
pub const BASE_VAR: &str = "HOME";

/// `%LOCALAPPDATA%\Sonara`, if `LOCALAPPDATA` is set and not empty.
pub fn windows_home(localappdata: Option<&str>) -> Option<PathBuf> {
    localappdata
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join("Sonara"))
}

/// `$HOME/Library/Application Support/Sonara`, if `HOME` is set and not
/// empty. The bash launchers and both SDKs build the same path.
pub fn macos_home(home: Option<&str>) -> Option<PathBuf> {
    home.filter(|d| !d.is_empty()).map(|d| {
        Path::new(d)
            .join("Library")
            .join("Application Support")
            .join("Sonara")
    })
}

/// This platform's default home from the value of `BASE_VAR`:
/// `%LOCALAPPDATA%\Sonara` on Windows, else
/// `$HOME/Library/Application Support/Sonara`.
pub fn default_home(base: Option<&str>) -> Option<PathBuf> {
    if cfg!(windows) {
        windows_home(base)
    } else {
        macos_home(base)
    }
}

/// The home from the two environment values: `SONARA_HOME` when set and
/// not empty, else the default home built on `base` (`BASE_VAR`).
pub fn home_from(sonara_home: Option<&str>, base: Option<&str>) -> Option<PathBuf> {
    match sonara_home.filter(|h| !h.is_empty()) {
        Some(h) => Some(PathBuf::from(h)),
        None => default_home(base),
    }
}

/// The runtime's home, read through `env` (`home_from`, as `sonarad`).
pub fn home(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    home_from(env("SONARA_HOME").as_deref(), env(BASE_VAR).as_deref())
}

/// Whether the user shut Sonara down in `home` (`STOPPED`).
pub fn stopped(home: &Path) -> bool {
    home.join(STOPPED).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
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

    #[cfg(not(windows))]
    #[test]
    fn sonara_home_beats_the_application_support_folder() {
        let env = |k: &str| match k {
            "HOME" => Some("/Users/x".to_string()),
            "LOCALAPPDATA" => Some("/ignored".to_string()),
            _ => None,
        };
        assert_eq!(
            home(&env),
            Some(PathBuf::from("/Users/x/Library/Application Support/Sonara"))
        );
        let env2 = |k: &str| match k {
            "SONARA_HOME" => Some("/h".to_string()),
            "HOME" => Some("/Users/x".to_string()),
            _ => None,
        };
        assert_eq!(home(&env2), Some(PathBuf::from("/h")));
        assert_eq!(home(&|_| None), None);
        assert_eq!(
            home_from(Some(""), Some("/Users/x")),
            Some(PathBuf::from("/Users/x/Library/Application Support/Sonara"))
        );
        assert_eq!(home_from(None, Some("")), None);
    }

    #[test]
    fn each_platform_builds_its_default_home() {
        assert_eq!(windows_home(Some("L")), Some(Path::new("L").join("Sonara")));
        assert_eq!(windows_home(Some("")), None);
        assert_eq!(
            macos_home(Some("H")),
            Some(
                Path::new("H")
                    .join("Library")
                    .join("Application Support")
                    .join("Sonara")
            )
        );
        assert_eq!(macos_home(None), None);
        let expected = if cfg!(windows) {
            windows_home(Some("B"))
        } else {
            macos_home(Some("B"))
        };
        assert_eq!(default_home(Some("B")), expected);
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
