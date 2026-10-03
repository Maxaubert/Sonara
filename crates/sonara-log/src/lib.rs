//! Sonara's troubleshooting log (#219): every log file of a home lives in
//! `<home>\logs`, written by `sonarad` (many threads) and by every
//! `sonara-hook.exe` process at once.
//!
//! - **Streams and segments.** A stream (`sonarad`, `hook`) appends to
//!   `<stream>.log`. A line that would take it past the segment size
//!   (`SEGMENT_BYTES`, about 1 MB) first rotates it: `<stream>.<n>.log`
//!   becomes `<stream>.<n+1>.log` (oldest last) and the file becomes
//!   `<stream>.1.log`.
//! - **One budget for the folder.** Everything in `logs\` together (every
//!   stream, every segment, files other writers left there such as the
//!   bootstrap's) stays at or under `BUDGET_BYTES` (10 MB): a line that
//!   would pass it first deletes the oldest files (by last write, so FIFO
//!   across streams), and as a last resort empties the stream's own file
//!   (when a viewer holding a segment stopped its rotation). A line that
//!   still cannot fit is dropped, never written past the budget.
//! - **Process and thread safe.** Each append takes an OS lock on
//!   `logs\.lock` (`File::try_lock`, retried until the writer's wait
//!   runs out), then checks sizes, rotates, prunes and appends the whole
//!   line with one write. So concurrent writers never tear, lose or
//!   duplicate a line across a rotation. A writer that cannot get the lock
//!   in time skips its line (`Error::Busy`): logging never holds up a hook.
//! - **One line per entry.** Line breaks become spaces and a line longer
//!   than `MAX_LINE_BYTES` is clipped with a `...[+N bytes]` marker.
//!
//! Best effort by design: every failure is an `Err` the caller may ignore.
//! No dependencies, so the hook adapter (L5) links it without any runtime
//! crate.
use std::borrow::Cow;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A stream's file rotates before it would pass this size.
pub const SEGMENT_BYTES: u64 = 1_000_000;
/// Everything in the log folder together stays at or under this size.
pub const BUDGET_BYTES: u64 = 10_000_000;
/// A longer line is clipped.
pub const MAX_LINE_BYTES: usize = 256 * 1024;
/// The lock file every writer takes (empty; never pruned).
pub const LOCK_FILE: &str = ".lock";
/// How long a writer waits for the lock by default: short, since
/// `sonarad` writes lines while it holds its channel and agent locks (a
/// writer holds the log lock for one append only).
pub const DEFAULT_WAIT: Duration = Duration::from_millis(50);

/// Why a line was not written.
#[derive(Debug)]
pub enum Error {
    /// Another writer held the lock for longer than this writer waits.
    Busy,
    /// The line does not fit in the budget even with every other file
    /// deleted (or the files could not be deleted).
    OverBudget,
    /// A bad stream name (letters, digits, `-` and `_` only).
    BadStream,
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Busy => f.write_str("the log is busy"),
            Error::OverBudget => f.write_str("the line does not fit in the log budget"),
            Error::BadStream => f.write_str("bad log stream name"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Serializes this process's writers before the OS lock, so threads queue
/// on a mutex instead of polling the file lock.
static IN_PROCESS: Mutex<()> = Mutex::new(());

/// One log folder and its limits.
#[derive(Debug, Clone)]
pub struct LogDir {
    dir: PathBuf,
    segment: u64,
    budget: u64,
    wait: Duration,
}

/// A file in the folder, for the budget.
struct Found {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
    /// The segment number of a `<stream>.<n>.log` (older first on a tie).
    number: u32,
}

fn valid_stream(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The segment number of `name` if it is `<stream>.<n>.log`.
fn segment_number(name: &str, stream: &str) -> Option<u32> {
    let rest = name.strip_prefix(stream)?.strip_prefix('.')?;
    let n = rest.strip_suffix(".log")?;
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

/// Any `<x>.<n>.log`'s number, for the tie-break.
fn any_segment_number(name: &str) -> u32 {
    name.strip_suffix(".log")
        .and_then(|s| s.rsplit_once('.'))
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(0)
}

impl LogDir {
    /// The folder `dir` with the default limits (`SEGMENT_BYTES`,
    /// `BUDGET_BYTES`, `DEFAULT_WAIT`).
    pub fn new(dir: impl Into<PathBuf>) -> LogDir {
        LogDir {
            dir: dir.into(),
            segment: SEGMENT_BYTES,
            budget: BUDGET_BYTES,
            wait: DEFAULT_WAIT,
        }
    }

    /// Other limits (tests).
    pub fn with_limits(mut self, segment: u64, budget: u64) -> LogDir {
        self.segment = segment.max(1);
        self.budget = budget.max(1);
        self
    }

    /// How long to wait for another writer before skipping the line.
    pub fn with_wait(mut self, wait: Duration) -> LogDir {
        self.wait = wait;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn budget(&self) -> u64 {
        self.budget
    }

    /// `<stream>.log`, the file a stream appends to.
    pub fn path(&self, stream: &str) -> PathBuf {
        self.dir.join(format!("{stream}.log"))
    }

    /// `<stream>.<n>.log`, an older segment (1 is the newest).
    pub fn segment_path(&self, stream: &str, n: u32) -> PathBuf {
        self.dir.join(format!("{stream}.{n}.log"))
    }

    /// Append `line` with a UTC timestamp in front (`timestamp`).
    pub fn log(&self, stream: &str, line: &str) -> Result<(), Error> {
        self.append(stream, &format!("{} {line}", timestamp(SystemTime::now())))
    }

    /// Append `line` (made one line and clipped) to `stream` under the
    /// rules of the module docs.
    pub fn append(&self, stream: &str, line: &str) -> Result<(), Error> {
        if !valid_stream(stream) {
            return Err(Error::BadStream);
        }
        let mut bytes = one_line(line).into_owned().into_bytes();
        bytes.push(b'\n');
        let _threads = IN_PROCESS.lock().unwrap_or_else(|p| p.into_inner());
        let lock = self.lock()?;
        let result = self.append_locked(stream, &bytes);
        let _ = lock.unlock();
        result
    }

    /// Total size of every file in the folder (the lock file is empty).
    pub fn total(&self) -> u64 {
        self.files().iter().map(|f| f.len).sum()
    }

    fn lock(&self) -> Result<File, Error> {
        std::fs::create_dir_all(&self.dir)?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join(LOCK_FILE))?;
        let end = Instant::now() + self.wait;
        loop {
            match f.try_lock() {
                Ok(()) => return Ok(f),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(e)) => return Err(Error::Io(e)),
            }
            if Instant::now() >= end {
                return Err(Error::Busy);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Every file in the folder but the lock file.
    fn files(&self) -> Vec<Found> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        rd.filter_map(Result::ok)
            .filter(|e| e.file_name() != LOCK_FILE)
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                if !m.is_file() {
                    return None;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                Some(Found {
                    path: e.path(),
                    len: m.len(),
                    modified: m.modified().unwrap_or(UNIX_EPOCH),
                    number: any_segment_number(&name),
                })
            })
            .collect()
    }

    fn append_locked(&self, stream: &str, bytes: &[u8]) -> Result<(), Error> {
        let n = bytes.len() as u64;
        if n > self.budget {
            return Err(Error::OverBudget);
        }
        let cur = self.path(stream);
        let len = std::fs::metadata(&cur).map(|m| m.len()).unwrap_or(0);
        if len > 0 && len + n > self.segment {
            self.rotate(stream, &cur);
        }
        let mut files = self.files();
        let mut total: u64 = files.iter().map(|f| f.len).sum();
        if total + n > self.budget {
            files.retain(|f| f.path != cur);
            files.sort_by(|a, b| a.modified.cmp(&b.modified).then(b.number.cmp(&a.number)));
            for f in files {
                if total + n <= self.budget {
                    break;
                }
                if std::fs::remove_file(&f.path).is_ok() {
                    total -= f.len;
                }
            }
            if total + n > self.budget {
                // Last resort: the current file could not rotate (a viewer
                // holds a segment) and fills the budget by itself. Empty
                // it rather than refuse every later line of the stream.
                let len = std::fs::metadata(&cur).map(|m| m.len()).unwrap_or(0);
                if len > 0 {
                    if let Ok(f) = OpenOptions::new().write(true).open(&cur) {
                        if f.set_len(0).is_ok() {
                            total -= len;
                        }
                    }
                }
            }
            if total + n > self.budget {
                return Err(Error::OverBudget);
            }
        }
        let mut f = OpenOptions::new().create(true).append(true).open(&cur)?;
        f.write_all(bytes)?;
        Ok(())
    }

    /// Shift the stream's segments up by one and make `cur` segment 1. A
    /// segment that cannot move (a viewer holds it) stops the rotation, so
    /// nothing is overwritten; `cur` then keeps growing, still under the
    /// budget, and is emptied when it alone fills it (`append_locked`).
    fn rotate(&self, stream: &str, cur: &Path) {
        let mut numbers: Vec<u32> = std::fs::read_dir(&self.dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .filter_map(|e| segment_number(&e.file_name().to_string_lossy(), stream))
                    .collect()
            })
            .unwrap_or_default();
        numbers.sort_unstable_by(|a, b| b.cmp(a));
        for n in numbers {
            let to = self.segment_path(stream, n.saturating_add(1));
            if std::fs::rename(self.segment_path(stream, n), to).is_err() {
                return;
            }
        }
        let first = self.segment_path(stream, 1);
        if std::fs::rename(cur, &first).is_err() {
            // Held without delete sharing: copy it out and empty it. When
            // it cannot be emptied the copy goes again, so no line is in
            // two files.
            if std::fs::copy(cur, &first).is_ok() {
                let emptied = OpenOptions::new()
                    .write(true)
                    .open(cur)
                    .and_then(|f| f.set_len(0))
                    .is_ok();
                if !emptied {
                    let _ = std::fs::remove_file(&first);
                }
            }
        }
    }
}

/// `line` as one line (line breaks become spaces), clipped to
/// `MAX_LINE_BYTES`.
pub fn one_line(line: &str) -> Cow<'_, str> {
    let clipped = clip(line, MAX_LINE_BYTES);
    if clipped.contains(['\n', '\r']) {
        Cow::Owned(clipped.replace(['\n', '\r'], " "))
    } else {
        clipped
    }
}

/// `s` cut to at most `max` bytes (on a character boundary) with a
/// `...[+N bytes]` marker naming what was cut, or `s` as it is.
pub fn clip(s: &str, max: usize) -> Cow<'_, str> {
    if s.len() <= max {
        return Cow::Borrowed(s);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}...[+{} bytes]", &s[..end], s.len() - end))
}

/// UTC time as `2026-10-03T12:34:56.789Z`.
pub fn timestamp(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, day) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
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

    #[test]
    fn clipping_keeps_characters_whole_and_names_the_rest() {
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("abcdef", 3), "abc...[+3 bytes]");
        // 'e' with an accent is two bytes: the cut moves back to a boundary.
        assert_eq!(clip("\u{e9}\u{e9}", 3), "\u{e9}...[+2 bytes]");
        assert_eq!(one_line("a\r\nb\nc"), "a  b c");
    }

    #[test]
    fn timestamps_are_utc_with_milliseconds() {
        let t = UNIX_EPOCH + Duration::from_millis(1_759_494_896_789);
        assert_eq!(timestamp(t), "2025-10-03T12:34:56.789Z");
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn segment_names() {
        assert_eq!(segment_number("hook.3.log", "hook"), Some(3));
        assert_eq!(segment_number("hook.log", "hook"), None);
        assert_eq!(segment_number("hook.old.log", "hook"), None);
        assert_eq!(segment_number("hooks.1.log", "hook"), None);
        assert_eq!(any_segment_number("sonarad.12.log"), 12);
        assert_eq!(any_segment_number("bootstrap.log"), 0);
        assert!(valid_stream("sonarad") && valid_stream("hook_2"));
        assert!(!valid_stream("a.b") && !valid_stream("") && !valid_stream("../x"));
    }
}
