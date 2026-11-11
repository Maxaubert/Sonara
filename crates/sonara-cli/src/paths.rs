//! Where the home and the runtime folders are.
use std::path::{Path, PathBuf};

/// The folder of the installed runtimes under the default home
/// (`%LOCALAPPDATA%\Sonara`, on macOS `~/Library/Application Support/Sonara`).
pub const RUNTIME_DIR: &str = "runtime";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `SONARA_HOME`, else the default home.
    pub home: PathBuf,
    /// `<default home>/runtime` (one folder per version).
    pub runtime_root: Option<PathBuf>,
    /// The folder of `sonara` (`sonarad` and `sonara-hook` are next to it).
    pub exe_dir: PathBuf,
}

/// The runtime folders stay under the default home whatever `SONARA_HOME`
/// says: the plugin installs them there.
pub fn runtime_root(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    sonara_client::default_home(env(sonara_client::BASE_VAR).as_deref())
        .map(|d| d.join(RUNTIME_DIR))
}

pub fn resolve(env: &dyn Fn(&str) -> Option<String>, exe: &Path) -> Result<Paths, String> {
    let home = sonara_client::home(env).ok_or_else(|| {
        format!(
            "no home folder: set SONARA_HOME or {}",
            sonara_client::BASE_VAR
        )
    })?;
    Ok(Paths {
        home,
        runtime_root: runtime_root(env),
        exe_dir: exe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    })
}

/// Comparable form of a path that exists.
pub fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Whether `inner` is `outer` or inside it.
pub fn is_within(inner: &Path, outer: &Path) -> bool {
    canonical(inner).starts_with(canonical(outer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_home_and_the_runtime_folders_come_from_the_environment() {
        let env = |k: &str| match k {
            "LOCALAPPDATA" => Some(r"C:\L".to_string()),
            _ => None,
        };
        let p = resolve(&env, Path::new(r"C:\L\Sonara\runtime\0.11.0\sonara.exe")).unwrap();
        assert_eq!(p.home, PathBuf::from(r"C:\L\Sonara"));
        assert_eq!(p.runtime_root, Some(PathBuf::from(r"C:\L\Sonara\runtime")));
        assert_eq!(p.exe_dir, PathBuf::from(r"C:\L\Sonara\runtime\0.11.0"));
        let env = |k: &str| match k {
            "SONARA_HOME" => Some(r"D:\h".to_string()),
            _ => None,
        };
        let p = resolve(&env, Path::new("sonara.exe")).unwrap();
        assert_eq!(p.home, PathBuf::from(r"D:\h"));
        assert_eq!(p.runtime_root, None);
        assert!(resolve(&|_: &str| None, Path::new("x")).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn the_home_and_the_runtime_folders_come_from_the_environment() {
        let env = |k: &str| match k {
            "HOME" => Some("/Users/x".to_string()),
            _ => None,
        };
        let sup = "/Users/x/Library/Application Support/Sonara";
        let p = resolve(&env, Path::new(&format!("{sup}/runtime/0.11.0/sonara"))).unwrap();
        assert_eq!(p.home, PathBuf::from(sup));
        assert_eq!(
            p.runtime_root,
            Some(PathBuf::from(format!("{sup}/runtime")))
        );
        assert_eq!(p.exe_dir, PathBuf::from(format!("{sup}/runtime/0.11.0")));
        let env = |k: &str| match k {
            "SONARA_HOME" => Some("/h".to_string()),
            _ => None,
        };
        let p = resolve(&env, Path::new("sonara")).unwrap();
        assert_eq!(p.home, PathBuf::from("/h"));
        assert_eq!(p.runtime_root, None);
        assert!(resolve(&|_: &str| None, Path::new("x")).is_err());
    }
}
