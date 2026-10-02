//! Where the home and the runtime folders are.
use std::path::{Path, PathBuf};

/// The folder of the installed runtimes under `%LOCALAPPDATA%\Sonara`.
pub const RUNTIME_DIR: &str = "runtime";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `SONARA_HOME`, else `%LOCALAPPDATA%\Sonara`.
    pub home: PathBuf,
    /// `%LOCALAPPDATA%\Sonara\runtime` (one folder per version).
    pub runtime_root: Option<PathBuf>,
    /// The folder of `sonara.exe` (`sonarad.exe` and `sonara-hook.exe`
    /// are next to it).
    pub exe_dir: PathBuf,
}

pub fn runtime_root(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("LOCALAPPDATA")
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(&d).join("Sonara").join(RUNTIME_DIR))
}

pub fn resolve(env: &dyn Fn(&str) -> Option<String>, exe: &Path) -> Result<Paths, String> {
    let home = sonara_hook::home(env).ok_or("no home folder: set SONARA_HOME or LOCALAPPDATA")?;
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
}
