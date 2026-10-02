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
//!   lookup (pid, then process name); what still fails stays recorded
//!   (`pending`, and the file) for the next restore and the startup sweep.
//!   A fresh duck of an app with a pending record keeps that record's
//!   original. Each recorded session gets its own original back, also when
//!   one app has several sessions (one per render device).
//! - The audio engine and virtual routers are never ducked: their session
//!   is the whole mix, Sonara's own speech included.
//!
//! Best-effort: nothing here returns an error to speech; failures are
//! logged.
use crate::platform::{AudioSession, AudioSessions};
use crate::state_file;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

/// What the crash-restore file holds per lowered session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub original: f32,
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
        }
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
                            log(&format!("session error while ducking: {e}"));
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
                    let rec = Record {
                        pid: Some(pid),
                        name: if name.is_empty() { None } else { Some(name) },
                        original: carried.map_or(original, |i| self.pending[i].original),
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
                        log(&format!("session error while ducking: {e}"));
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
                log(&format!("cannot enumerate audio sessions: {e}"));
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
        if !saved.is_empty() {
            let lowered: Vec<Record> = saved.iter().map(|(_, r)| r.clone()).collect();
            log(&format!("lowered {}", names(&lowered)));
        }
        self.saved = saved;
    }

    /// The startup sweep (`restore_from_state_file`); what still fails is
    /// kept as pending, so the next restore retries it and a later duck
    /// keeps it in the file.
    pub fn recover(&mut self) {
        restore_from_state_file(self.sessions.as_ref(), &self.state);
        if let Some(file) = state_file::read::<StateFile>(&self.state) {
            for r in file.sessions {
                if !self.pending.contains(&r) {
                    self.pending.push(r);
                }
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
                Ok(left) => failed = left,
                Err(e) => log(&format!("cannot enumerate audio sessions for restore: {e}")),
            }
        }
        self.ducked = false;
        if failed.is_empty() {
            state_file::clear(&self.state);
        } else {
            write_state(&self.state, &failed);
            log(&format!("restore failed for {}", names(&failed)));
        }
        self.pending = failed;
    }
}

/// Restore recorded sessions by a fresh enumeration, matched by pid, then
/// by process name (the session object may be stale, or the app
/// restarted). A pid that now belongs to another app is not used. Returns
/// the records that still failed; a record with no live session is dropped
/// (its process is gone). Errs only when enumeration fails.
pub fn restore_records(
    sessions: &dyn AudioSessions,
    records: &[Record],
) -> Result<Vec<Record>, String> {
    let live = sessions.sessions()?;
    let mut done: HashSet<usize> = HashSet::new();
    let mut failed: Vec<usize> = Vec::new();
    for s in live {
        let pid = s.pid();
        let name = s.name();
        // Each live session takes a record no earlier session used: one
        // app with several sessions (one per render device) has a record
        // per session, each with its own original.
        let unused = |i: &usize| !done.contains(i) && !failed.contains(i);
        let by_pid = (0..records.len()).filter(unused).find(|&i| {
            let r = &records[i];
            r.pid == Some(pid)
                && match (&r.name, name.is_empty()) {
                    // L-duck-pid: a reused pid belongs to another app now.
                    (Some(n), false) if !n.is_empty() => n.eq_ignore_ascii_case(&name),
                    _ => true,
                }
        });
        let idx = by_pid.or_else(|| {
            if name.is_empty() {
                return None;
            }
            (0..records.len())
                .filter(unused)
                .find(|&i| records[i].name.as_deref() == Some(name.as_str()))
        });
        let Some(i) = idx else { continue };
        match s.set_volume(records[i].original) {
            Ok(()) => {
                done.insert(i);
            }
            Err(_) => failed.push(i),
        }
    }
    let mut out = Vec::new();
    for i in failed {
        if !done.contains(&i) && !out.iter().any(|r: &Record| r == &records[i]) {
            out.push(records[i].clone());
        }
    }
    Ok(out)
}

/// The startup crash sweep: if a previous runtime died while ducked (or a
/// restore failed), restore every live session matching a record. Records
/// that still fail stay in the file; if enumeration fails the file is kept
/// untouched.
pub fn restore_from_state_file(sessions: &dyn AudioSessions, path: &Path) {
    let Some(file) = state_file::read::<StateFile>(path) else {
        return;
    };
    match restore_records(sessions, &file.sessions) {
        Ok(left) if left.is_empty() => state_file::clear(path),
        Ok(left) => {
            write_state(path, &left);
            log(&format!("startup restore failed for {}", names(&left)));
        }
        Err(e) => log(&format!(
            "startup restore could not enumerate sessions: {e}"
        )),
    }
}
