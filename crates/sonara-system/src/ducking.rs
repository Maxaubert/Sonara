//! Duck other apps: lower every other app's audio session to a level while
//! speech plays, then put each back (ported from the Python
//! `platform/windows/ducking.py`, rules #130 and #131).
//!
//! - Every session is handled on its own: one failing session (a virtual
//!   device invalidated mid-enumeration) never strands the ones already
//!   lowered. Whatever was lowered is recorded, in memory and in the
//!   crash-restore file, before anything else can go wrong.
//! - A session already at or below the duck level is left alone and never
//!   recorded: its level would be saved as the "original" (the
//!   stuck-at-30% bug).
//! - A restore that fails on the session object is retried by a fresh
//!   lookup (session identity, then pid, then process name, each pass over
//!   every record before the next; a session not at the level Sonara set
//!   is not taken, #14); what still fails stays recorded (`pending`, and
//!   the file) for the next restore and the startup sweep. A fresh duck of
//!   an app with a pending record keeps that record's original. Each
//!   recorded session gets its own original back, also when one app has
//!   several sessions (one per render device).
//! - A record whose app is not running is kept (up to `RECORD_MAX_AGE`)
//!   and applied when its app's session shows up in a later duck or
//!   restore (#15): Windows reopens the app at the level it was left at.
//! - The audio engine and virtual routers are never ducked: their session
//!   is the whole mix, Sonara's own speech included.
//!
//! Best-effort: nothing here returns an error to speech; failures are
//! logged.
use crate::log::{self as activity, LogFn};
use crate::platform::{AudioSession, AudioSessions, PResult};
use crate::state_file;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Processes whose session is the aggregated output to the hardware
/// (lower-cased image names): the Windows audio engine and common per-app
/// virtual routers (SteelSeries Sonar, VoiceMeeter).
pub const NEVER_DUCK: &[&str] = &[
    "audiodg.exe",
    "steelseriessonar.exe",
    "voicemeeter.exe",
    "voicemeeter8.exe",
    "voicemeeter8x64.exe",
];

/// How long a record waits for its app to come back (#15): Windows keeps
/// an app's session volume across restarts, so an app that was ducked when
/// the runtime died reopens at the duck level.
pub const RECORD_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// What the crash-restore file holds per lowered session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub original: f32,
    /// The session's instance identity, matched first (#14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The level Sonara set: a session at another level is not this one,
    /// or was changed since (#14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ducked: Option<f32>,
    /// When it was recorded (Unix seconds), for `RECORD_MAX_AGE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
}

/// Unix seconds now.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Whether `level` is the level `r` was ducked to (no recorded level, or a
/// level that cannot be read, counts as a match).
fn at_duck_level(r: &Record, level: &PResult<f32>) -> bool {
    match (r.ducked, level) {
        (Some(d), Ok(v)) => (d - v).abs() < 0.01,
        _ => true,
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    sessions: Vec<Record>,
}

fn log(message: &str) {
    eprintln!("sonara-system: [duck] {message}");
}

fn names(records: &[Record]) -> String {
    records
        .iter()
        .map(|r| match (&r.name, r.pid) {
            (Some(n), _) if !n.is_empty() => n.clone(),
            (_, Some(p)) => p.to_string(),
            _ => "?".to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The app names of `records` (process name, else pid), each once.
pub fn app_names(records: &[Record]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in records {
        let n = match (&r.name, r.pid) {
            (Some(n), _) if !n.is_empty() => n.clone(),
            (_, Some(p)) => p.to_string(),
            _ => "?".to_string(),
        };
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

fn write_state(path: &Path, records: &[Record]) {
    let file = StateFile {
        sessions: records.to_vec(),
    };
    if let Err(e) = state_file::write(path, &file) {
        log(&format!("cannot write {}: {e}", path.display()));
    }
}

fn same_app(r: &Record, pid: u32, name: &str) -> bool {
    r.pid == Some(pid) && r.name.as_deref().unwrap_or("") == name
}

/// Lowers and restores other apps' sessions; one per runtime.
pub struct Ducker {
    sessions: Box<dyn AudioSessions>,
    state: PathBuf,
    /// Sessions lowered by the current duck, with what to restore.
    saved: Vec<(Box<dyn AudioSession>, Record)>,
    /// Records whose restore failed; retried by the next restore.
    pending: Vec<Record>,
    ducked: bool,
    /// Where failures and the startup sweep are reported (#217).
    log: Option<LogFn>,
}

impl Ducker {
    /// `state` is the crash-restore file (`<home>\state\duck_state.json`).
    pub fn new(sessions: Box<dyn AudioSessions>, state: PathBuf) -> Self {
        Ducker {
            sessions,
            state,
            saved: Vec::new(),
            pending: Vec::new(),
            ducked: false,
            log: None,
        }
    }

    /// Report failures and the startup sweep to `log` (else stderr).
    pub fn with_log(mut self, log: Option<LogFn>) -> Self {
        self.log = log;
        self
    }

    fn note(&self, line: &str) {
        activity::emit(self.log.as_ref(), line);
    }

    pub fn is_ducked(&self) -> bool {
        self.ducked
    }

    /// The platform sessions this ducker works on.
    pub fn sessions(&self) -> &dyn AudioSessions {
        self.sessions.as_ref()
    }

    /// The records of the sessions the current duck lowered.
    pub fn saved(&self) -> Vec<Record> {
        self.saved.iter().map(|(_, r)| r.clone()).collect()
    }

    /// Records still waiting for a successful restore.
    pub fn pending(&self) -> &[Record] {
        &self.pending
    }

    /// Lower every session except `exclude` pids and `NEVER_DUCK` to
    /// `level` percent. A no-op while ducked.
    pub fn duck(&mut self, exclude: &[u32], level: u8) {
        if self.ducked {
            return;
        }
        // A pending record's app may be running again (#15): put it back
        // first, so it is ducked from its true level like any other app.
        if !self.pending.is_empty() {
            if let Ok(sweep) = restore_records(self.sessions.as_ref(), &self.pending) {
                self.pending = sweep.left();
            }
        }
        let target = f32::from(level.min(100)) / 100.0;
        let mut saved: Vec<(Box<dyn AudioSession>, Record)> = Vec::new();
        // Pending records a fresh duck of the same session took over.
        let mut taken: HashSet<usize> = HashSet::new();
        let enumerated = match self.sessions.sessions() {
            Ok(list) => {
                for s in list {
                    let pid = s.pid();
                    let name = s.name();
                    if exclude.contains(&pid) || NEVER_DUCK.contains(&name.to_lowercase().as_str())
                    {
                        continue;
                    }
                    let original = match s.volume() {
                        Ok(v) => v,
                        Err(e) => {
                            self.note(&format!("duck failed app={}: {e}", activity::value(&name)));
                            continue;
                        }
                    };
                    if original <= target + 0.005 {
                        // Already at or below the duck level: nothing to
                        // lower, and recording it would save a ducked
                        // level as the original.
                        continue;
                    }
                    // An app with a pending record is still at an old duck
                    // level: its true original is the pending one, never
                    // the level it is stuck at (the stuck-at-30% bug).
                    let carried = (0..self.pending.len())
                        .find(|i| !taken.contains(i) && same_app(&self.pending[*i], pid, &name));
                    let id = s.id();
                    let rec = Record {
                        pid: Some(pid),
                        name: if name.is_empty() { None } else { Some(name) },
                        original: carried.map_or(original, |i| self.pending[i].original),
                        id: if id.is_empty() { None } else { Some(id) },
                        ducked: Some(target),
                        at: Some(now_secs()),
                    };
                    // On disk before the volume moves: a runtime killed
                    // right after lowering it never strands this app.
                    let mut records: Vec<Record> = (0..self.pending.len())
                        .filter(|i| !taken.contains(i) && Some(*i) != carried)
                        .map(|i| self.pending[i].clone())
                        .collect();
                    records.extend(saved.iter().map(|(_, r)| r.clone()));
                    records.push(rec.clone());
                    write_state(&self.state, &records);
                    if let Err(e) = s.set_volume(target) {
                        self.note(&format!(
                            "duck failed app={}: {e}",
                            activity::value(rec.name.as_deref().unwrap_or(""))
                        ));
                        continue;
                    }
                    if let Some(i) = carried {
                        taken.insert(i);
                    }
                    saved.push((s, rec));
                }
                true
            }
            Err(e) => {
                self.note(&format!(
                    "duck failed: cannot enumerate audio sessions: {e}"
                ));
                false
            }
        };
        // Enumeration failed and nothing was lowered: stay un-ducked so the
        // next call retries.
        self.ducked = enumerated || !saved.is_empty();
        // A pending record a fresh duck took over now lives in `saved`,
        // with its original carried along.
        let mut i = 0;
        self.pending.retain(|_| {
            i += 1;
            !taken.contains(&(i - 1))
        });
        let records: Vec<Record> = self
            .pending
            .iter()
            .cloned()
            .chain(saved.iter().map(|(_, r)| r.clone()))
            .collect();
        if records.is_empty() {
            state_file::clear(&self.state);
        } else {
            write_state(&self.state, &records);
        }
        self.saved = saved;
    }

    /// The startup sweep (`restore_from_state_file`); what still fails is
    /// kept as pending, so the next restore retries it and a later duck
    /// keeps it in the file.
    pub fn recover(&mut self) {
        let left = match restore_from_state_file(self.sessions.as_ref(), &self.state) {
            Some(sweep) => {
                if !sweep.restored.is_empty() {
                    self.note(&format!(
                        "startup sweep: restore apps={}",
                        activity::apps(&app_names(&sweep.restored))
                    ));
                }
                if !sweep.failed.is_empty() {
                    self.note(&format!(
                        "startup sweep: restore failed apps={}",
                        activity::apps(&app_names(&sweep.failed))
                    ));
                }
                if !sweep.waiting.is_empty() {
                    self.note(&format!(
                        "startup sweep: restore waits for apps={} (not running)",
                        activity::apps(&app_names(&sweep.waiting))
                    ));
                }
                sweep.left()
            }
            // Nothing could be listed: the file is kept as it was.
            None => state_file::read::<StateFile>(&self.state)
                .map(|f| f.sessions)
                .unwrap_or_default(),
        };
        for r in left {
            if !self.pending.contains(&r) {
                self.pending.push(r);
            }
        }
    }

    /// Put every lowered session back. What cannot be restored stays in
    /// `pending` and the file.
    pub fn restore(&mut self) {
        let mut failed = std::mem::take(&mut self.pending);
        for (s, rec) in std::mem::take(&mut self.saved) {
            if s.set_volume(rec.original).is_err() {
                failed.push(rec);
            }
        }
        if !failed.is_empty() {
            match restore_records(self.sessions.as_ref(), &failed) {
                Ok(sweep) => {
                    if !sweep.failed.is_empty() {
                        self.note(&format!(
                            "restore failed apps={}",
                            activity::apps(&app_names(&sweep.failed))
                        ));
                    }
                    failed = sweep.left();
                }
                Err(e) => self.note(&format!(
                    "restore failed: cannot enumerate audio sessions: {e}"
                )),
            }
        }
        self.ducked = false;
        if failed.is_empty() {
            state_file::clear(&self.state);
        } else {
            write_state(&self.state, &failed);
        }
        self.pending = failed;
    }
}

/// What `restore_records` did with each record.
#[derive(Debug, Default)]
pub struct Sweep {
    /// Put back (or found changed since, so left as it is).
    pub restored: Vec<Record>,
    /// Matched a live session whose `set_volume` failed: kept.
    pub failed: Vec<Record>,
    /// No live session (the app is not running): kept for when it is back.
    pub waiting: Vec<Record>,
    /// Older than `RECORD_MAX_AGE` with nothing to restore: dropped.
    pub expired: Vec<Record>,
}

impl Sweep {
    /// The records to keep: failed, then waiting.
    pub fn left(&self) -> Vec<Record> {
        self.failed
            .iter()
            .chain(self.waiting.iter())
            .cloned()
            .collect()
    }
}

/// Restore recorded sessions by a fresh enumeration (the session object
/// may be stale, or the app restarted). Every record is matched in three
/// passes, each over all records before the next, so a weaker match never
/// takes a session a stronger one wants (#14): the session identity, then
/// the pid (a pid that now belongs to another app is not used), then the
/// process name. A session not at the level the record was ducked to is
/// not taken by pid or name, and one found by identity at another level
/// was changed since and is left alone. A record with no live session is
/// kept as waiting (#15) until it is older than `RECORD_MAX_AGE`; a record
/// without a time is stamped now. Errs only when enumeration fails.
pub fn restore_records(sessions: &dyn AudioSessions, records: &[Record]) -> Result<Sweep, String> {
    let live = sessions.sessions()?;
    let levels: Vec<PResult<f32>> = live.iter().map(|s| s.volume()).collect();
    let names: Vec<String> = live.iter().map(|s| s.name()).collect();
    let ids: Vec<String> = live.iter().map(|s| s.id()).collect();
    // Which live session each record took.
    let mut pick: Vec<Option<usize>> = vec![None; records.len()];
    let mut used: HashSet<usize> = HashSet::new();
    let mut take = |pick: &mut Vec<Option<usize>>, ok: &dyn Fn(&Record, usize) -> bool| {
        for (i, r) in records.iter().enumerate() {
            if pick[i].is_some() {
                continue;
            }
            if let Some(j) = (0..live.len()).find(|j| !used.contains(j) && ok(r, *j)) {
                used.insert(j);
                pick[i] = Some(j);
            }
        }
    };
    take(&mut pick, &|r, j| {
        r.id.as_deref()
            .is_some_and(|id| !id.is_empty() && id == ids[j])
    });
    take(&mut pick, &|r, j| {
        r.pid == Some(live[j].pid())
            && match (&r.name, names[j].is_empty()) {
                // L-duck-pid: a reused pid belongs to another app now.
                (Some(n), false) if !n.is_empty() => n.eq_ignore_ascii_case(&names[j]),
                _ => true,
            }
            && at_duck_level(r, &levels[j])
    });
    take(&mut pick, &|r, j| {
        !names[j].is_empty()
            && r.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(&names[j]))
            && at_duck_level(r, &levels[j])
    });
    let now = now_secs();
    let max = RECORD_MAX_AGE.as_secs();
    let mut out = Sweep::default();
    for (i, r) in records.iter().enumerate() {
        let mut r = r.clone();
        let at = *r.at.get_or_insert(now);
        match pick[i] {
            // Found by its identity at another level: changed since.
            Some(j) if !at_duck_level(&r, &levels[j]) => out.restored.push(r),
            Some(j) => match live[j].set_volume(r.original) {
                Ok(()) => out.restored.push(r),
                Err(_) if now.saturating_sub(at) > max => out.expired.push(r),
                Err(_) => out.failed.push(r),
            },
            None if now.saturating_sub(at) > max => out.expired.push(r),
            None => out.waiting.push(r),
        }
    }
    Ok(out)
}

/// The startup crash sweep: if a previous runtime died while ducked (or a
/// restore failed), restore every live session matching a record. Records
/// that still fail, or whose app is not running, stay in the file; if
/// enumeration fails the file is kept untouched (and `None` returned).
pub fn restore_from_state_file(sessions: &dyn AudioSessions, path: &Path) -> Option<Sweep> {
    let file = state_file::read::<StateFile>(path)?;
    match restore_records(sessions, &file.sessions) {
        Ok(sweep) => {
            let left = sweep.left();
            if left.is_empty() {
                state_file::clear(path);
            } else {
                write_state(path, &left);
            }
            if !sweep.failed.is_empty() {
                log(&format!(
                    "startup restore failed for {}",
                    names(&sweep.failed)
                ));
            }
            Some(sweep)
        }
        Err(e) => {
            log(&format!(
                "startup restore could not enumerate sessions: {e}"
            ));
            None
        }
    }
}
