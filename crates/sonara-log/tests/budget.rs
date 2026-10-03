//! The log folder's rules (#219): rotation into numbered segments, one
//! size budget over the whole folder with the oldest files deleted first,
//! and writers in several processes that never tear, lose or duplicate a
//! line across a rotation.
use sonara_log::{Error, LogDir, LOCK_FILE};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Set in a child of this test binary: `<dir>|<stream>|<lines>|<segment>|<budget>`.
const CHILD: &str = "SONARA_LOG_TEST_CHILD";

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sonara-log-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// Every line in the folder, oldest file first.
fn all_lines(dir: &Path) -> Vec<String> {
    let mut files: Vec<(std::time::SystemTime, u32, PathBuf)> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != LOCK_FILE)
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name
                .strip_suffix(".log")
                .and_then(|s| s.rsplit_once('.'))
                .and_then(|(_, n)| n.parse().ok())
                .unwrap_or(0u32);
            (e.metadata().unwrap().modified().unwrap(), n, e.path())
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    files
        .iter()
        .flat_map(|(_, _, p)| read(p).lines().map(str::to_string).collect::<Vec<_>>())
        .collect()
}

#[test]
fn a_full_stream_rotates_into_numbered_segments() {
    let dir = tmp("rotate");
    let log = LogDir::new(&dir).with_limits(35, 10_000);
    // 10 bytes a line: three fit in a 35-byte segment.
    for i in 0..10 {
        log.append("a", &format!("line {i:04}")).unwrap();
    }
    assert_eq!(
        read(&log.segment_path("a", 3)),
        "line 0000\nline 0001\nline 0002\n"
    );
    assert_eq!(
        read(&log.segment_path("a", 2)),
        "line 0003\nline 0004\nline 0005\n"
    );
    assert_eq!(
        read(&log.segment_path("a", 1)),
        "line 0006\nline 0007\nline 0008\n"
    );
    assert_eq!(read(&log.path("a")), "line 0009\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_budget_holds_over_many_small_and_large_lines_and_drops_the_oldest_first() {
    let dir = tmp("budget");
    let log = LogDir::new(&dir).with_limits(1_000, 5_000);
    // A file another writer left (the bootstrap's) counts and goes first.
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bootstrap.log"), "x".repeat(800)).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let mut seq = 0;
    for round in 0..300 {
        let stream = if round % 3 == 0 { "hook" } else { "sonarad" };
        let size = [5usize, 40, 300, 900, 12][round % 5];
        let line = format!("{stream} {seq:05} {}", "y".repeat(size));
        seq += 1;
        log.append(stream, &line).unwrap();
        assert!(log.total() <= 5_000, "over the budget after line {seq}");
    }
    assert!(
        !dir.join("bootstrap.log").exists(),
        "the oldest file went first"
    );
    // What is left of each stream is its newest lines with none missing in
    // between (whole files go, oldest last write first: FIFO).
    let mut by_stream: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for l in all_lines(&dir) {
        let mut parts = l.split(' ');
        let stream = parts.next().unwrap().to_string();
        let seq = parts.next().unwrap().parse().unwrap();
        by_stream.entry(stream).or_default().push(seq);
    }
    assert_eq!(by_stream.len(), 2, "both streams kept their newest lines");
    for (stream, seqs) in by_stream {
        let last_written = (0..300u32)
            .rev()
            .find(|i| (*i % 3 == 0) == (stream == "hook"))
            .unwrap();
        assert_eq!(*seqs.last().unwrap(), last_written, "{stream}");
        let expected: Vec<u32> = (seqs[0]..=last_written)
            .filter(|i| (*i % 3 == 0) == (stream == "hook"))
            .collect();
        assert_eq!(seqs, expected, "{stream}: a contiguous tail, in order");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_line_bigger_than_the_budget_is_refused_and_a_huge_one_clipped() {
    let dir = tmp("huge");
    let log = LogDir::new(&dir).with_limits(1_000, 2_000);
    assert!(matches!(
        log.append("a", &"z".repeat(3_000)),
        Err(Error::OverBudget)
    ));
    assert_eq!(log.total(), 0);
    let big = LogDir::new(&dir);
    big.append("b", &"q".repeat(400_000)).unwrap();
    let text = read(&big.path("b"));
    assert!(text.len() < 300_000, "{}", text.len());
    assert!(text.trim_end().ends_with("bytes]"), "the clip is marked");
    assert!(matches!(log.append("a.b", "x"), Err(Error::BadStream)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_held_lock_makes_a_writer_skip_its_line_in_time() {
    let dir = tmp("busy");
    std::fs::create_dir_all(&dir).unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))
        .unwrap();
    held.lock().unwrap();
    let log = LogDir::new(&dir).with_wait(Duration::from_millis(50));
    let t = std::time::Instant::now();
    assert!(matches!(log.append("hook", "skipped"), Err(Error::Busy)));
    assert!(t.elapsed() < Duration::from_secs(2));
    held.unlock().unwrap();
    log.append("hook", "written").unwrap();
    assert_eq!(read(&log.path("hook")), "written\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Run as a child process (see `CHILD`); a no-op in a normal test run.
#[test]
fn child_writer() {
    let Ok(spec) = std::env::var(CHILD) else {
        return;
    };
    let parts: Vec<&str> = spec.split('|').collect();
    let (dir, stream) = (parts[0], parts[1]);
    let lines: u32 = parts[2].parse().unwrap();
    let segment: u64 = parts[3].parse().unwrap();
    let budget: u64 = parts[4].parse().unwrap();
    let log = LogDir::new(dir)
        .with_limits(segment, budget)
        .with_wait(Duration::from_secs(10));
    for i in 0..lines {
        // Lines of different sizes, so rotations land mid-run.
        let pad = "p".repeat((i as usize * 37) % 200);
        log.append(stream, &format!("{stream} {i:05} {pad} end"))
            .unwrap();
        assert!(log.total() <= budget, "over the budget");
    }
}

fn run_children(dir: &Path, lines: u32, segment: u64, budget: u64) {
    let me = std::env::current_exe().unwrap();
    let children: Vec<_> = ["w1", "w2"]
        .iter()
        .map(|w| {
            Command::new(&me)
                .args(["child_writer", "--exact", "--nocapture", "--test-threads=1"])
                .env(
                    CHILD,
                    format!("{}|{w}|{lines}|{segment}|{budget}", dir.display()),
                )
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success(), "a writer failed");
    }
}

/// Each writer's sequence numbers, from every line, checking each line is
/// whole.
fn by_writer(dir: &Path) -> BTreeMap<String, Vec<u32>> {
    let mut out: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for line in all_lines(dir) {
        let parts: Vec<&str> = line.split(' ').collect();
        assert!(
            parts.len() == 4 && parts[3] == "end" && parts[2].chars().all(|c| c == 'p'),
            "torn line: {line:?}"
        );
        out.entry(parts[0].to_string())
            .or_default()
            .push(parts[1].parse().unwrap());
    }
    out
}

#[test]
fn two_writer_processes_lose_and_duplicate_nothing_across_rotations() {
    let dir = tmp("procs");
    // Small segments, a budget nothing reaches: every line must be there.
    run_children(&dir, 400, 4_000, 1_000_000);
    let got = by_writer(&dir);
    for w in ["w1", "w2"] {
        let mut seqs = got[w].clone();
        seqs.sort_unstable();
        assert_eq!(
            seqs,
            (0..400).collect::<Vec<_>>(),
            "{w}: lost or duplicated"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_writer_processes_stay_under_the_budget_and_keep_a_fifo_tail() {
    let dir = tmp("procs-budget");
    run_children(&dir, 600, 3_000, 12_000);
    assert!(LogDir::new(&dir).total() <= 12_000);
    for (w, mut seqs) in by_writer(&dir) {
        seqs.sort_unstable();
        let (first, last) = (seqs[0], *seqs.last().unwrap());
        assert_eq!(last, 599, "{w}: its newest line is kept");
        assert_eq!(
            seqs,
            (first..=last).collect::<Vec<_>>(),
            "{w}: only the oldest lines go"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
