//! Start and stop the runtime, and remove the folders of older releases.
use crate::client::{attach, Conn, PRODUCT};
use crate::paths::{canonical, Paths};
use crate::VERSION;
use serde_json::json;
use sonara_hook::{read_runtime, start_runtime, Runtime, PROBE, RUNTIME_EXE, STOPPED};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a start may take to answer (a first start also migrates the
/// Python plugin's settings and restores apps a dead runtime left).
pub const START_WAIT: Duration = Duration::from_secs(20);
/// How long a stopped runtime may take to exit.
pub const STOP_WAIT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

/// A runtime of this release, connected and armed (`keep_alive`).
pub struct Running {
    pub rt: Runtime,
    pub conn: Conn,
    pub version: String,
    /// Started by this command (it was not running).
    pub started: bool,
    /// The version of another release's runtime that was replaced.
    pub replaced: Option<String>,
}

fn version_of(hello: &serde_json::Value) -> String {
    hello["version"].as_str().unwrap_or("").to_string()
}

/// Ask the runtime on `conn` to exit (`shutdown`, extension `system`).
pub fn shutdown(conn: &mut Conn) -> Result<(), String> {
    let r = conn
        .request(json!({"type": "shutdown"}))
        .map_err(|e| e.to_string())?;
    if r["ok"] == true {
        Ok(())
    } else {
        Err(r["error"]["message"]
            .as_str()
            .unwrap_or("refused")
            .to_string())
    }
}

/// Wait until the runtime `rt` no longer answers (and its `runtime.json`
/// is gone or names another process).
pub fn wait_gone(home: &Path, rt: &Runtime, timeout: Duration) -> Result<(), String> {
    let end = Instant::now() + timeout;
    loop {
        let same_file = read_runtime(home).is_some_and(|now| now.pid == rt.pid);
        if !same_file || Conn::open(rt, PROBE).is_err() {
            return Ok(());
        }
        if Instant::now() >= end {
            return Err(format!(
                "the runtime (pid {}) did not exit within {} s",
                rt.pid.unwrap_or(0),
                timeout.as_secs()
            ));
        }
        std::thread::sleep(POLL);
    }
}

/// Make sure this release's runtime runs and is armed: clear the stop
/// sentinel, replace a runtime of another release, start `sonarad.exe`
/// from `exe_dir` (`--standalone` plus `extra`) when none answers.
pub fn ensure_running(home: &Path, exe_dir: &Path, extra: &[String]) -> Result<Running, String> {
    let _ = std::fs::remove_file(home.join(STOPPED));
    let mut replaced = None;
    if let Some((rt, mut conn, h)) = attach(home, PRODUCT, true) {
        let version = version_of(&h);
        if version == VERSION {
            return Ok(Running {
                rt,
                conn,
                version,
                started: false,
                replaced: None,
            });
        }
        // An upgrade: the runtime of the previous release is still up.
        shutdown(&mut conn)
            .map_err(|e| format!("Sonara {version} is running and did not stop: {e}"))?;
        drop(conn);
        wait_gone(home, &rt, STOP_WAIT)?;
        replaced = Some(version);
    }
    let exe = exe_dir.join(RUNTIME_EXE);
    if !exe.is_file() {
        return Err(format!("{} is missing", exe.display()));
    }
    std::fs::create_dir_all(home).map_err(|e| format!("cannot create {}: {e}", home.display()))?;
    let mut args = vec!["--standalone".to_string()];
    args.extend(extra.iter().cloned());
    let mut child =
        start_runtime(&exe, &args).map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    let end = Instant::now() + START_WAIT;
    while Instant::now() < end {
        std::thread::sleep(POLL);
        if let Some((rt, conn, h)) = attach(home, PRODUCT, true) {
            return Ok(Running {
                rt,
                conn,
                version: version_of(&h),
                started: true,
                replaced,
            });
        }
        // Exit code 3: another instance holds the home; keep waiting for
        // its runtime.json. Any other exit is a failed start.
        if let Ok(Some(status)) = child.try_wait() {
            if !matches!(status.code(), Some(3) | Some(0)) {
                return Err(format!(
                    "sonarad.exe exited ({status}); see {}",
                    home.join("logs").join("sonarad.log").display()
                ));
            }
        }
    }
    Err(format!(
        "the runtime did not answer within {} s",
        START_WAIT.as_secs()
    ))
}

/// What `stop` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// The runtime with this pid exited.
    Exited(Option<u64>),
    /// No runtime was running.
    NotRunning,
}

/// Write the stop sentinel, then end the running runtime, if any.
pub fn stop(home: &Path) -> Result<Stopped, String> {
    std::fs::create_dir_all(home)
        .and_then(|_| std::fs::write(home.join(STOPPED), b""))
        .map_err(|e| format!("cannot write {}: {e}", home.join(STOPPED).display()))?;
    let Some((rt, mut conn, _)) = attach(home, &["system"], false) else {
        return Ok(Stopped::NotRunning);
    };
    shutdown(&mut conn)?;
    drop(conn);
    wait_gone(home, &rt, STOP_WAIT)?;
    Ok(Stopped::Exited(rt.pid))
}

/// The version folders in `root` other than `keep` (temporary folders,
/// whose names start with a dot, are the bootstrap's).
pub fn old_versions(root: &Path, keep: &Path) -> Vec<PathBuf> {
    let keep = canonical(keep);
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .filter(|p| canonical(p) != keep)
        .collect();
    out.sort();
    out
}

/// Remove the folders of older releases, when this `sonara.exe` runs from
/// a version folder of the runtime root. Returns what was removed and
/// what could not be (a file still in use; the next start tries again).
pub fn remove_old_versions(paths: &Paths) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    let Some(root) = &paths.runtime_root else {
        return (removed, failed);
    };
    let ours = paths.exe_dir.parent().map(canonical);
    if ours.as_deref() != Some(canonical(root).as_path()) {
        return (removed, failed);
    }
    for dir in old_versions(root, &paths.exe_dir) {
        let mut result = std::fs::remove_dir_all(&dir);
        for _ in 0..10 {
            if result.is_ok() || !dir.exists() {
                break;
            }
            // A runtime that just exited can hold its files a moment.
            std::thread::sleep(Duration::from_millis(200));
            result = std::fs::remove_dir_all(&dir);
        }
        match result {
            Ok(()) => removed.push(dir),
            Err(_) if !dir.exists() => removed.push(dir),
            Err(e) => failed.push((dir, e.to_string())),
        }
    }
    (removed, failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "sonara-cli-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn older_version_folders_are_removed_and_ours_and_temp_folders_stay() {
        let lad = tmp();
        let root = lad.join("Sonara").join("runtime");
        for v in ["0.10.0", "0.11.0", ".staging-1"] {
            std::fs::create_dir_all(root.join(v)).unwrap();
            std::fs::write(root.join(v).join("sonara.exe"), b"x").unwrap();
        }
        let paths = Paths {
            home: lad.join("Sonara"),
            runtime_root: Some(root.clone()),
            exe_dir: root.join("0.11.0"),
        };
        let (removed, failed) = remove_old_versions(&paths);
        assert_eq!(removed, vec![root.join("0.10.0")]);
        assert!(failed.is_empty());
        assert!(root.join("0.11.0").is_dir());
        assert!(root.join(".staging-1").is_dir());
    }

    #[test]
    fn nothing_is_removed_when_not_running_from_the_runtime_root() {
        let lad = tmp();
        let root = lad.join("Sonara").join("runtime");
        std::fs::create_dir_all(root.join("0.10.0")).unwrap();
        let paths = Paths {
            home: lad.join("Sonara"),
            runtime_root: Some(root.clone()),
            exe_dir: lad.join("target"),
        };
        assert_eq!(remove_old_versions(&paths), (vec![], vec![]));
        assert!(root.join("0.10.0").is_dir());
    }

    #[test]
    fn stop_without_a_runtime_writes_the_sentinel() {
        let home = tmp();
        assert_eq!(stop(&home).unwrap(), Stopped::NotRunning);
        assert!(home.join(STOPPED).is_file());
    }
}
