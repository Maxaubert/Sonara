//! Small JSON files in the home: written atomically (a temp file, then a
//! rename), read leniently. Used for the crash-restore records and the
//! keymap.
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

/// Write `value` as pretty JSON to `path` through a temp file and a rename,
/// creating the folder. A reader that holds the file open can make the
/// rename fail for a moment on Windows, so it is retried briefly.
pub fn write<T: Serialize + ?Sized>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    let mut last = None;
    for _ in 0..20 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_file(&tmp);
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

/// The file's JSON, or `None` when it is missing or unreadable.
pub fn read<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Remove the file; a missing file is fine.
pub fn clear(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_clear_round_trip() {
        let dir = std::env::temp_dir().join(format!("sonara-sf-{}", std::process::id()));
        let path = dir.join("nested").join("x.json");
        write(&path, &serde_json::json!({"a": 1})).unwrap();
        let v: serde_json::Value = read(&path).unwrap();
        assert_eq!(v["a"], 1);
        clear(&path);
        assert!(read::<serde_json::Value>(&path).is_none());
        clear(&path);
        let _ = std::fs::remove_dir_all(dir);
    }
}
