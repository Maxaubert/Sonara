//! Start and stop the runtime, and remove the folders of older releases.
use crate::client::{attach, Conn, PRODUCT};
use crate::paths::{canonical, Paths};
use crate::VERSION;
use serde_json::json;
use sonara_client::{read_runtime, start_runtime, Runtime, PROBE, RUNTIME_EXE, STOPPED};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a start may take to answer (a first start also migrates the
/// Python plugin's settings and restores apps a dead runtime left).
pub const START_WAIT: Duration = Duration::from_secs(20);
/// How long a stopped runtime may take to exit.
pub const STOP_WAIT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

/// A runtime of this release or a newer one, connected and armed
/// (`keep_alive`).
pub struct Running {
    pub rt: Runtime,
    pub conn: Conn,
    pub version: String,
    /// Started by this command (it was not running).
    pub started: bool,
    /// The version of an older release's runtime that was replaced.
    pub replaced: Option<String>,
}

fn version_of(hello: &serde_json::Value) -> String {
    hello["version"].as_str().unwrap_or("").to_string()
}

/// `major.minor.patch` of a release version (a `-` or `+` suffix is
/// ignored); `None` for anything else.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let out = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(out)
}

/// `v` is a release older than `than` (both must parse).
pub fn is_older(v: &str, than: &str) -> bool {
    matches!((parse_version(v), parse_version(than)), (Some(a), Some(b)) if a < b)
}

/// A running runtime of version `running` is replaced by release `ours`
/// only when it is older (or names no version). Upgrades go one way:
/// sessions of an older plugin keep running after a plugin update, and
/// their `start` attaches to the newer runtime instead of replacing it.
pub fn replaces(running: &str, ours: &str) -> bool {
    running != ours && (parse_version(running).is_none() || is_older(running, ours))
}

/// A runtime of release `ours` serves a plugin that needs `wanted`: the
/// same release or a newer one (the plugin's launcher uses the newest
/// runtime installed).
pub fn serves(wanted: &str, ours: &str) -> bool {
    wanted == ours || is_older(wanted, ours)
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

/// Make sure a runtime of this release or a newer one runs and is armed:
/// clear the stop sentinel, replace a runtime of an older release, start
/// `sonarad.exe` from `exe_dir` (`--standalone` plus `extra`) when none
/// answers.
pub fn ensure_running(home: &Path, exe_dir: &Path, extra: &[String]) -> Result<Running, String> {
    let _ = std::fs::remove_file(home.join(STOPPED));
    let mut replaced = None;
    if let Some((rt, mut conn, h)) = attach(home, PRODUCT, true) {
        let version = version_of(&h);
        if !replaces(&version, VERSION) {
            return Ok(Running {
                rt,
                conn,
                version,
                started: false,
                replaced: None,
            });
        }
        // An upgrade: the runtime of an older release is still up.
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

/// The version folders in `root` of releases older than `keep`'s (or
/// this release, when `keep` is not named after a version). Newer folders
/// stay: an older plugin's session may still run beside a newer one, and
/// other names (the bootstrap's dot folders) are not ours to remove.
pub fn old_versions(root: &Path, keep: &Path) -> Vec<PathBuf> {
    let ours = keep
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| parse_version(n).is_some())
        .unwrap_or_else(|| VERSION.to_string());
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter(|e| is_older(&e.file_name().to_string_lossy(), &ours))
        .map(|e| e.path())
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
    fn newer_version_folders_and_other_names_are_kept() {
        // An older plugin's `sonara start` must not remove the runtime a
        // newer plugin installed (sessions of both run side by side).
        let lad = tmp();
        let root = lad.join("Sonara").join("runtime");
        for v in ["0.9.9", "0.11.0", "0.12.0", "1.0.0", "notes"] {
            std::fs::create_dir_all(root.join(v)).unwrap();
        }
        let paths = Paths {
            home: lad.join("Sonara"),
            runtime_root: Some(root.clone()),
            exe_dir: root.join("0.11.0"),
        };
        let (removed, failed) = remove_old_versions(&paths);
        assert_eq!(removed, vec![root.join("0.9.9")]);
        assert!(failed.is_empty());
        for v in ["0.11.0", "0.12.0", "1.0.0", "notes"] {
            assert!(root.join(v).is_dir(), "{v} kept");
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_older("0.9.0", "0.11.0"));
        assert!(is_older("0.11.0", "0.11.1"));
        assert!(is_older("0.11.9", "1.0.0"));
        assert!(!is_older("0.11.0", "0.11.0"));
        assert!(!is_older("0.12.0", "0.11.0"));
        assert!(!is_older("0.11.0-dev", "0.11.0"), "a suffix is ignored");
        assert_eq!(parse_version("0.11"), None);
        assert_eq!(parse_version("x.1.2"), None);
    }

    #[test]
    fn only_an_older_or_unknown_runtime_is_replaced() {
        // Upgrades go one way: a newer runtime is attached to, never shut
        // down by an older plugin's start.
        assert!(replaces("0.10.0", "0.11.0"));
        assert!(replaces("", "0.11.0"));
        assert!(!replaces("0.11.0", "0.11.0"));
        assert!(!replaces("0.12.0", "0.11.0"));
    }

    #[test]
    fn a_newer_runtime_serves_an_older_plugin() {
        assert!(serves("0.11.0", "0.11.0"));
        assert!(
            !serves("0.11.0", "0.10.0"),
            "this runtime is older than the plugin's"
        );
        assert!(serves("0.11.0", "0.12.0"));
        assert!(!serves("garbage", "0.12.0"));
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
