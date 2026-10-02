//! `sonara uninstall`: remove the runtime folders and the home, except
//! what the user keeps, and leave the stop sentinel.
use crate::paths::is_within;
use sonara_hook::STOPPED;
use std::path::{Path, PathBuf};

/// What the user may keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// `config.json`, `keymap.json`, `session_prefs.json`.
    Settings,
    /// `models\` (the Kokoro voice model, about 350 MB).
    Models,
    /// `logs\`.
    Logs,
}

pub const DEFAULT_KEEP: &[Keep] = &[Keep::Settings];

impl Keep {
    fn names(self) -> &'static [&'static str] {
        match self {
            Keep::Settings => &["config.json", "keymap.json", "session_prefs.json"],
            Keep::Models => &["models"],
            Keep::Logs => &["logs"],
        }
    }
}

/// `--keep settings,models,logs` (any of them), or `none`.
pub fn parse_keep(list: &str) -> Result<Vec<Keep>, String> {
    let mut out = Vec::new();
    for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let k = match item.to_ascii_lowercase().as_str() {
            "none" => continue,
            "settings" => Keep::Settings,
            "models" | "model" | "voices" => Keep::Models,
            "logs" => Keep::Logs,
            other => {
                return Err(format!(
                    "unknown --keep item '{other}' (use settings, models, logs or none)"
                ))
            }
        };
        if !out.contains(&k) {
            out.push(k);
        }
    }
    Ok(out)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub removed: Vec<PathBuf>,
    pub kept: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    /// A folder removed once this process has exited (it holds the
    /// running `sonara.exe`).
    pub deferred: Option<PathBuf>,
}

fn remove(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Remove `folder` after this process has exited: a hidden `cmd` waits a
/// moment, then deletes it.
#[cfg(windows)]
fn remove_later(folder: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let line = format!(
        "/d /c ping -n 4 127.0.0.1 >nul & rd /s /q \"{}\"",
        folder.display()
    );
    std::process::Command::new("cmd.exe")
        .raw_arg(line)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
fn remove_later(_folder: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("not supported"))
}

/// Remove the runtime folders (`runtime_root`) and everything in `home`
/// except what `keep` names; then write the stop sentinel, so the plugin's
/// hooks do not install the runtime again until `/sonara:start`.
pub fn remove_all(home: &Path, runtime_root: Option<&Path>, exe: &Path, keep: &[Keep]) -> Report {
    let mut report = Report::default();
    let kept: Vec<&str> = keep.iter().flat_map(|k| k.names()).copied().collect();
    if let Some(root) = runtime_root.filter(|r| r.exists()) {
        if is_within(exe, root) {
            match remove_later(root) {
                Ok(()) => report.deferred = Some(root.to_path_buf()),
                Err(e) => report.failed.push((root.to_path_buf(), e.to_string())),
            }
        } else {
            match remove(root) {
                Ok(()) => report.removed.push(root.to_path_buf()),
                Err(e) => report.failed.push((root.to_path_buf(), e.to_string())),
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(home) {
        let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if name == STOPPED {
                continue;
            }
            if runtime_root.is_some_and(|r| is_within(&p, r) || is_within(r, &p)) {
                continue;
            }
            if kept.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
                report.kept.push(p);
                continue;
            }
            match remove(&p) {
                Ok(()) => report.removed.push(p),
                Err(e) => report.failed.push((p, e.to_string())),
            }
        }
    }
    let _ = std::fs::create_dir_all(home).and_then(|_| std::fs::write(home.join(STOPPED), b""));
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "sonara-uninstall-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_keep_list_parses() {
        assert_eq!(
            parse_keep("settings, models").unwrap(),
            vec![Keep::Settings, Keep::Models]
        );
        assert_eq!(parse_keep("none").unwrap(), vec![]);
        assert_eq!(parse_keep("logs,logs").unwrap(), vec![Keep::Logs]);
        assert!(parse_keep("everything").is_err());
    }

    #[test]
    fn everything_but_the_kept_files_goes_and_the_sentinel_stays() {
        let lad = tmp();
        let home = lad.join("Sonara");
        let root = home.join("runtime");
        std::fs::create_dir_all(root.join("0.11.0")).unwrap();
        std::fs::create_dir_all(home.join("models").join("kokoro")).unwrap();
        std::fs::create_dir_all(home.join("logs")).unwrap();
        std::fs::create_dir_all(home.join("state")).unwrap();
        for f in [
            "config.json",
            "keymap.json",
            "runtime.json",
            "session_prefs.json",
        ] {
            std::fs::write(home.join(f), b"{}").unwrap();
        }
        let exe = lad.join("elsewhere").join("sonara.exe");
        let r = remove_all(&home, Some(&root), &exe, &[Keep::Settings, Keep::Logs]);
        assert!(r.failed.is_empty(), "{:?}", r.failed);
        assert!(!root.exists() && !home.join("models").exists() && !home.join("state").exists());
        assert!(!home.join("runtime.json").exists());
        for kept in ["config.json", "keymap.json", "session_prefs.json", "logs"] {
            assert!(home.join(kept).exists(), "{kept}");
        }
        assert!(home.join(STOPPED).is_file());
        assert!(r.removed.contains(&root));
        assert_eq!(r.kept.len(), 4);
    }
}
