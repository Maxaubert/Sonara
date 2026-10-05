//! `runtime.json` (spec section 3): how clients find and authenticate to the
//! running instance. Written atomically (a temp file in the home, made
//! user-only, then renamed over the old one) and removed on a clean exit.
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub pid: u32,
    pub port: u16,
    pub http_port: u16,
    pub token: String,
    pub version: String,
    pub protocol: (u64, u64),
    pub capabilities: Vec<String>,
    /// Extensions a client may enable in `hello`.
    pub extensions: Vec<String>,
    /// RFC 3339, UTC.
    pub started_at: String,
}

impl RuntimeInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "pid": self.pid,
            "port": self.port,
            "http_port": self.http_port,
            "token": self.token,
            "version": self.version,
            "protocol": {"major": self.protocol.0, "minor": self.protocol.1},
            "capabilities": self.capabilities,
            "extensions": self.extensions,
            "started_at": self.started_at,
        })
    }
}

/// `restrict` makes a file user-only (`instance::restrict_to_user`).
pub fn write(
    path: &Path,
    info: &RuntimeInfo,
    restrict: impl Fn(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let tmp = temp_path(path, info.pid);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        // Restrict before the token is written.
        restrict(&tmp)?;
        let body = serde_json::to_string_pretty(&info.to_json()).map_err(|e| e.to_string())?;
        f.write_all(body.as_bytes())
            .and_then(|_| f.sync_all())
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        drop(f);
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("cannot move {} into place: {e}", tmp.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn temp_path(path: &Path, pid: u32) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{pid}.tmp"));
    path.with_file_name(name)
}

/// The `pid` in the file, if it parses. Clients read the same file through
/// `sonara_client::read_runtime` (#255), which needs a valid port and
/// token; here only the pid matters (is the file still this process's).
pub fn read_pid(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("pid")?.as_u64().map(|p| p as u32)
}

/// Remove the file if it still describes this process (a later instance may
/// already have replaced it).
pub fn remove_if_ours(path: &Path, pid: u32) {
    if read_pid(path) == Some(pid) {
        let _ = std::fs::remove_file(path);
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a time after 1970.
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian
/// (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn info(pid: u32) -> RuntimeInfo {
        RuntimeInfo {
            pid,
            port: 1,
            http_port: 2,
            token: "t".into(),
            version: "0.0.0".into(),
            protocol: (1, 0),
            capabilities: vec!["core".into()],
            extensions: vec!["channels".into()],
            started_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sonarad-rt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn dates_format_as_rfc3339() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        assert_eq!(rfc3339(t), "2026-09-21T14:13:20Z");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(rfc3339(leap), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn write_replaces_atomically_and_remove_checks_the_pid() {
        let dir = temp_dir("write");
        let path = dir.join("runtime.json");
        std::fs::write(&path, "old").unwrap();
        write(&path, &info(42), |_| Ok(())).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["pid"], 42);
        assert_eq!(v["protocol"], json!({"major": 1, "minor": 0}));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp left");
        remove_if_ours(&path, 7);
        assert!(path.exists(), "another pid's file stays");
        remove_if_ours(&path, 42);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_restrict_leaves_nothing_behind() {
        let dir = temp_dir("fail");
        let path = dir.join("runtime.json");
        assert!(write(&path, &info(1), |_| Err("no".into())).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn the_file_is_user_only() {
        let dir = temp_dir("acl");
        let path = dir.join("runtime.json");
        let sid = crate::instance::user_sid().unwrap();
        write(&path, &info(1), |p| {
            crate::instance::restrict_to_user(p, &sid)
        })
        .unwrap();
        let (aces, protected) = crate::instance::dacl_summary(&path).unwrap();
        assert_eq!(aces, 1, "one entry: the user");
        assert!(protected, "no inherited entries");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
