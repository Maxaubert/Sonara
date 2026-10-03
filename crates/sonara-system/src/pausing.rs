//! Pause other apps' media while speech plays, then resume exactly those
//! (ported from the Python `platform/windows/pausing.py`, #92). Only real
//! media (apps with GSMTC transport controls) is affected; game sounds,
//! calls and notifications are not media sessions.
//!
//! Like ducking: a crash-restore file lists the apps paused, a resume that
//! fails keeps those apps in the file for the startup sweep (L-pause-state,
//! #131), and nothing here returns an error to speech.
use crate::log::{self as activity, LogFn};
use crate::platform::MediaSessions;
use crate::state_file;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    apps: Vec<String>,
}

fn log(message: &str) {
    eprintln!("sonara-system: [pause] {message}");
}

fn write_state(path: &Path, apps: &[String]) {
    let file = StateFile {
        apps: apps.to_vec(),
    };
    if let Err(e) = state_file::write(path, &file) {
        log(&format!("cannot write {}: {e}", path.display()));
    }
}

pub struct MediaPauser {
    media: Box<dyn MediaSessions>,
    state: PathBuf,
    paused_ids: Vec<String>,
    /// Apps whose resume failed (or the startup sweep could not resume):
    /// kept in the file and retried by the next resume.
    pending: Vec<String>,
    paused: bool,
    /// Where failures and the startup sweep are reported (#217).
    log: Option<LogFn>,
}

impl MediaPauser {
    /// `state` is the crash-restore file (`<home>\state\pause_state.json`).
    pub fn new(media: Box<dyn MediaSessions>, state: PathBuf) -> Self {
        MediaPauser {
            media,
            state,
            paused_ids: Vec::new(),
            pending: Vec::new(),
            paused: false,
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

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// The platform sessions this pauser works on.
    pub fn sessions(&self) -> &dyn MediaSessions {
        self.media.as_ref()
    }

    /// The apps the current pause paused.
    pub fn paused_apps(&self) -> &[String] {
        &self.paused_ids
    }

    /// The startup sweep (`resume_from_state_file`); what still fails is
    /// kept as pending.
    pub fn recover(&mut self) {
        let before = state_file::read::<StateFile>(&self.state)
            .map(|f| f.apps)
            .unwrap_or_default();
        resume_from_state_file(self.media.as_ref(), &self.state);
        let left = state_file::read::<StateFile>(&self.state)
            .map(|f| f.apps)
            .unwrap_or_default();
        let resumed: Vec<&String> = before.iter().filter(|a| !left.contains(a)).collect();
        if !resumed.is_empty() {
            self.note(&format!(
                "startup sweep: media resume apps={}",
                activity::apps(&resumed)
            ));
        }
        if !left.is_empty() {
            self.note(&format!(
                "startup sweep: media resume failed apps={}",
                activity::apps(&left)
            ));
        }
        for a in left {
            if !self.pending.contains(&a) {
                self.pending.push(a);
            }
        }
    }

    fn record(&self, ids: &[String]) {
        let mut all = self.pending.clone();
        all.extend(ids.iter().filter(|a| !self.pending.contains(a)).cloned());
        if all.is_empty() {
            state_file::clear(&self.state);
        } else {
            write_state(&self.state, &all);
        }
    }

    /// Pause every media session that is playing. A no-op while paused; when
    /// the sessions cannot be listed it stays un-paused so the next call
    /// retries.
    pub fn pause(&mut self) {
        if self.paused {
            return;
        }
        let list = match self.media.sessions() {
            Ok(l) => l,
            Err(e) => {
                self.note(&format!(
                    "media pause failed: cannot list media sessions: {e}"
                ));
                return;
            }
        };
        let mut ids: Vec<String> = Vec::new();
        for s in list {
            // An app we cannot name could not be resumed: leave it alone.
            let Ok(app) = s.app_id() else { continue };
            match s.is_playing() {
                Ok(true) => {}
                _ => continue,
            }
            if ids.contains(&app) {
                // Another session of an app already recorded.
                if let Err(e) = s.pause() {
                    self.note(&format!(
                        "media pause failed app={}: {e}",
                        activity::value(&app)
                    ));
                }
                continue;
            }
            // On disk before the app pauses: a runtime killed right after
            // never strands it paused.
            let mut with = ids.clone();
            with.push(app.clone());
            self.record(&with);
            match s.pause() {
                Ok(()) => ids.push(app),
                Err(e) => self.note(&format!(
                    "media pause failed app={}: {e}",
                    activity::value(&app)
                )),
            }
        }
        self.paused = true;
        self.record(&ids);
        self.paused_ids = ids;
    }

    /// Resume the apps the pause paused (a vanished one is skipped) and
    /// return the apps it tried. Apps whose resume failed, or all of them
    /// when the sessions cannot be listed, stay in the file for the startup
    /// sweep.
    pub fn resume(&mut self) -> Vec<String> {
        let mut wanted = std::mem::take(&mut self.pending);
        for a in std::mem::take(&mut self.paused_ids) {
            if !wanted.contains(&a) {
                wanted.push(a);
            }
        }
        self.paused = false;
        if wanted.is_empty() {
            state_file::clear(&self.state);
            return wanted;
        }
        let failed = resume_apps(self.media.as_ref(), &wanted).unwrap_or_else(|e| {
            self.note(&format!(
                "media resume failed: cannot list media sessions: {e}"
            ));
            wanted.clone()
        });
        if failed.is_empty() {
            state_file::clear(&self.state);
        } else {
            self.note(&format!(
                "media resume failed apps={}",
                activity::apps(&failed)
            ));
            write_state(&self.state, &failed);
        }
        self.pending = failed;
        wanted
    }
}

/// Play every live session of the `wanted` apps; returns the apps whose
/// play failed. An app with no live session is dropped (it is gone).
fn resume_apps(media: &dyn MediaSessions, wanted: &[String]) -> Result<Vec<String>, String> {
    let mut failed: Vec<String> = Vec::new();
    for s in media.sessions()? {
        let Ok(app) = s.app_id() else { continue };
        if !wanted.contains(&app) {
            continue;
        }
        if s.play().is_err() && !failed.contains(&app) {
            failed.push(app);
        }
    }
    Ok(failed)
}

/// The startup crash sweep: resume the apps a previous runtime paused and
/// could not resume. Apps whose resume fails stay in the file; if the
/// sessions cannot be listed the file is kept untouched.
pub fn resume_from_state_file(media: &dyn MediaSessions, path: &Path) {
    let Some(file) = state_file::read::<StateFile>(path) else {
        return;
    };
    if file.apps.is_empty() {
        state_file::clear(path);
        return;
    }
    match resume_apps(media, &file.apps) {
        Ok(left) if left.is_empty() => state_file::clear(path),
        Ok(left) => write_state(path, &left),
        Err(e) => log(&format!("startup resume could not list sessions: {e}")),
    }
}
