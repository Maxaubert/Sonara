//! Pause other apps' media while speech plays, then resume exactly those
//! (ported from the Python `platform/windows/pausing.py`, #92). Only real
//! media (apps with GSMTC transport controls) is affected; game sounds,
//! calls and notifications are not media sessions.
//!
//! Like ducking: a crash-restore file lists the apps paused, a resume that
//! fails keeps those apps in the file for the startup sweep (L-pause-state,
//! #131), and nothing here returns an error to speech.
//!
//! Resume plays the session objects the pause paused, never every session
//! of an app (#17): browser tabs share one app id, and a tab the user had
//! paused stays paused. A resume that failed is retried only while its
//! session is still paused and for `PENDING_MAX_AGE`; the startup sweep
//! (which only has app ids) ignores a record older than that.
use crate::log::{self as activity, LogFn};
use crate::platform::{self, MediaSession, MediaSessions};
use crate::state_file;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a resume that failed is retried, and how old a crash record
/// the startup sweep still resumes (#17): later, starting an app's media
/// would be unasked (a crash, then a reboot days later).
pub const PENDING_MAX_AGE: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    apps: Vec<String>,
    /// When the oldest entry was recorded (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<u64>,
}

fn log(message: &str) {
    eprintln!("sonara-system: [pause] {message}");
}

fn write_state(path: &Path, apps: &[String], at: u64) {
    let file = StateFile {
        apps: apps.to_vec(),
        at: Some(at),
    };
    if let Err(e) = state_file::write(path, &file) {
        log(&format!("cannot write {}: {e}", path.display()));
    }
}

fn now_secs() -> u64 {
    crate::ducking::now_secs()
}

fn expired(at: u64, now: u64) -> bool {
    now.saturating_sub(at) > PENDING_MAX_AGE.as_secs()
}

/// A media session with its app id.
type Held = (Box<dyn MediaSession>, String);

/// A resume still owed: the session itself when known (a failed play), or
/// only the app id (left by a previous runtime).
struct Pending {
    session: Option<Box<dyn MediaSession>>,
    app: String,
    at: u64,
}

fn unique(apps: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in apps {
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

pub struct MediaPauser {
    media: Box<dyn MediaSessions>,
    state: PathBuf,
    /// The sessions the current pause paused, with their app ids.
    saved: Vec<Held>,
    /// Resumes that failed (or the startup sweep could not do): kept in
    /// the file and retried by the next resume.
    pending: Vec<Pending>,
    /// When the pause was recorded (Unix seconds).
    since: u64,
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
            saved: Vec::new(),
            pending: Vec::new(),
            since: 0,
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

    /// The apps the current pause paused, each once.
    pub fn paused_apps(&self) -> Vec<String> {
        unique(self.saved.iter().map(|(_, a)| a.clone()))
    }

    /// The startup sweep (`resume_from_state_file`); what still fails is
    /// kept as pending.
    pub fn recover(&mut self) {
        let Some(file) = state_file::read::<StateFile>(&self.state) else {
            return;
        };
        let now = now_secs();
        let at = file.at.unwrap_or(now);
        match sweep(self.media.as_ref(), &self.state) {
            Some((resumed, failed)) => {
                if !resumed.is_empty() {
                    self.note(&format!(
                        "startup sweep: media resume apps={}",
                        activity::apps(&resumed)
                    ));
                }
                if !failed.is_empty() {
                    let apps = unique(failed.iter().map(|(_, a)| a.clone()));
                    self.note(&format!(
                        "startup sweep: media resume failed apps={}",
                        activity::apps(&apps)
                    ));
                }
                for (s, app) in failed {
                    self.pending.push(Pending {
                        session: Some(s),
                        app,
                        at,
                    });
                }
            }
            // The sessions cannot be listed: keep the apps for the next
            // resume (within the age limit).
            None if !expired(at, now) => {
                for app in file.apps {
                    self.pending.push(Pending {
                        session: None,
                        app,
                        at,
                    });
                }
            }
            None => {}
        }
    }

    /// The oldest time of what is recorded, or now.
    fn oldest(&self, now: u64) -> u64 {
        self.pending.iter().map(|p| p.at).min().unwrap_or(now)
    }

    fn record(&self, apps: &[String]) {
        let now = now_secs();
        let all = unique(
            self.pending
                .iter()
                .map(|p| p.app.clone())
                .chain(apps.iter().cloned()),
        );
        if all.is_empty() {
            state_file::clear(&self.state);
        } else {
            write_state(&self.state, &all, self.oldest(now));
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
        let mut saved: Vec<Held> = Vec::new();
        for s in list {
            // An app we cannot name could not be resumed: leave it alone.
            let Ok(app) = s.app_id() else { continue };
            match s.is_playing() {
                Ok(true) => {}
                _ => continue,
            }
            // On disk before the app pauses: a runtime killed right after
            // never strands it paused.
            let mut with: Vec<String> = saved.iter().map(|(_, a)| a.clone()).collect();
            with.push(app.clone());
            self.record(&with);
            match s.pause() {
                Ok(()) => saved.push((s, app)),
                Err(e) => {
                    self.note(&format!(
                        "media pause failed app={}: {e}",
                        activity::value(&app)
                    ));
                    // An app that answers late may pause after all (#16):
                    // resume it like one that paused.
                    if platform::timed_out(&e) {
                        saved.push((s, app));
                    }
                }
            }
        }
        self.paused = true;
        self.since = now_secs();
        let apps: Vec<String> = saved.iter().map(|(_, a)| a.clone()).collect();
        self.record(&apps);
        self.saved = saved;
    }

    /// Resume the sessions the pause paused (and what is pending) and
    /// return the apps it tried. A session playing again already is left
    /// alone; one that vanished is skipped. Apps whose resume failed, or
    /// all pending ones when the sessions cannot be listed, stay in the file
    /// for the startup sweep.
    pub fn resume(&mut self) -> Vec<String> {
        let now = now_secs();
        let since = if self.paused { self.since } else { now };
        self.paused = false;
        let mut todo: Vec<Pending> = std::mem::take(&mut self.saved)
            .into_iter()
            .map(|(s, app)| Pending {
                session: Some(s),
                app,
                at: since,
            })
            .collect();
        todo.extend(
            std::mem::take(&mut self.pending)
                .into_iter()
                .filter(|p| !expired(p.at, now)),
        );
        let wanted = unique(todo.iter().map(|p| p.app.clone()));
        if todo.is_empty() {
            state_file::clear(&self.state);
            return wanted;
        }
        let mut failed: Vec<Pending> = Vec::new();
        let mut by_app: Vec<Pending> = Vec::new();
        for p in todo {
            let Some(s) = &p.session else {
                by_app.push(p);
                continue;
            };
            match s.is_playing() {
                // Playing again (the user started it): leave it alone.
                Ok(true) => continue,
                // The session is gone (the app closed): nothing to resume.
                Err(_) => continue,
                Ok(false) => {}
            }
            if s.play().is_err() {
                failed.push(p);
            }
        }
        if !by_app.is_empty() {
            let apps = unique(by_app.iter().map(|p| p.app.clone()));
            match resume_apps(self.media.as_ref(), &apps) {
                Ok(left) => {
                    let at = by_app.iter().map(|p| p.at).min().unwrap_or(now);
                    failed.extend(left.into_iter().map(|(s, app)| Pending {
                        session: Some(s),
                        app,
                        at,
                    }));
                }
                Err(e) => {
                    self.note(&format!(
                        "media resume failed: cannot list media sessions: {e}"
                    ));
                    failed.extend(by_app);
                }
            }
        }
        self.pending = failed;
        if self.pending.is_empty() {
            state_file::clear(&self.state);
        } else {
            let apps = unique(self.pending.iter().map(|p| p.app.clone()));
            self.note(&format!(
                "media resume failed apps={}",
                activity::apps(&apps)
            ));
            write_state(&self.state, &apps, self.oldest(now));
        }
        wanted
    }
}

/// Play the paused live sessions of the `wanted` apps; returns the
/// sessions whose play failed. An app with no live session is dropped (it
/// is gone).
fn resume_apps(media: &dyn MediaSessions, wanted: &[String]) -> Result<Vec<Held>, String> {
    let mut failed = Vec::new();
    for s in media.sessions()? {
        let Ok(app) = s.app_id() else { continue };
        if !wanted.contains(&app) || matches!(s.is_playing(), Ok(true)) {
            continue;
        }
        if s.play().is_err() {
            failed.push((s, app));
        }
    }
    Ok(failed)
}

/// The sweep behind `resume_from_state_file`: the apps resumed and the
/// sessions that failed, `None` when nothing could be listed (the file is
/// kept as it was).
fn sweep(media: &dyn MediaSessions, path: &Path) -> Option<(Vec<String>, Vec<Held>)> {
    let file = state_file::read::<StateFile>(path)?;
    let now = now_secs();
    let at = file.at.unwrap_or(now);
    if file.apps.is_empty() || expired(at, now) {
        if !file.apps.is_empty() {
            log(&format!(
                "startup resume skipped: the record is older than {} s",
                PENDING_MAX_AGE.as_secs()
            ));
        }
        state_file::clear(path);
        return Some((Vec::new(), Vec::new()));
    }
    match resume_apps(media, &file.apps) {
        Ok(failed) => {
            let left = unique(failed.iter().map(|(_, a)| a.clone()));
            if left.is_empty() {
                state_file::clear(path);
            } else {
                write_state(path, &left, at);
            }
            let resumed = file
                .apps
                .iter()
                .filter(|a| !left.contains(a))
                .cloned()
                .collect();
            Some((resumed, failed))
        }
        Err(e) => {
            log(&format!("startup resume could not list sessions: {e}"));
            None
        }
    }
}

/// The startup crash sweep: resume the apps a previous runtime paused and
/// could not resume, unless the record is older than `PENDING_MAX_AGE`.
/// Apps whose resume fails stay in the file; if the sessions cannot be
/// listed the file is kept untouched.
pub fn resume_from_state_file(media: &dyn MediaSessions, path: &Path) {
    let _ = sweep(media, path);
}
