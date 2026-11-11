//! Pause the macOS Now Playing app (a YouTube video in Chrome or Safari,
//! Podcasts, VLC, any app that shows in Control Center's Now Playing)
//! while Sonara speaks, and resume only that media.
//!
//! **Why `osascript`.** Since macOS 15.4 MediaRemote (the private
//! framework behind Now Playing) answers only Apple's own programs: a
//! signed `sonarad` reads no now-playing item. `/usr/bin/osascript`
//! (`com.apple.osascript`) is one of Apple's programs, so a JavaScript for
//! Automation script it runs can load MediaRemote, read the now-playing
//! app and its state (`MRNowPlayingRequest`) and send it a command
//! (`MRMediaRemoteSendCommand`). It sends no Apple event, so there is no
//! Automation prompt, and nothing new is shipped or signed (the
//! `ungive/mediaremote-adapter` workaround does the same through
//! `/usr/bin/perl` and a framework of its own). Checked on macOS 27.2.
//!
//! **Targeting.** A MediaRemote command goes to whatever app is now
//! playing, not to a chosen one. So every command is one script that reads
//! the now-playing app and title and sends the command only when they are
//! still the ones expected, within milliseconds: `pause` only media that
//! plays, `play` only the app and title Sonara paused, and only while it
//! is paused (never media the user switched to, and never media that was
//! not playing). Any failure (the framework missing, a script error, a
//! call past `CALL_TIMEOUT`) leaves media alone and never holds speech
//! longer than one call's timeout.
//!
//! The apps the AppleScript path pauses (`MEDIA_APPS`, Music and Spotify)
//! are left to it, so nothing is paused twice.
use crate::mac::{osascript, CALL_TIMEOUT, MEDIA_APPS};
use crate::platform::{MediaSession, PResult};
use serde::Deserialize;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Runs a JavaScript for Automation script and returns its output (a seam:
/// the real one is `osascript` with `CALL_TIMEOUT`).
pub type Runner = Arc<dyn Fn(&str) -> PResult<String> + Send + Sync>;

/// The MediaRemote command ids (`MRCommand`).
pub const PLAY: u8 = 0;
pub const PAUSE: u8 = 1;

/// The real backend: `backend()` gives `snap` (the now-playing app, title
/// and state, or null), `send` (a MediaRemote command) and `wait` (run the
/// run loop, so a command goes out and its effect is seen). Tests swap in
/// a fake one with the same shape.
pub const MEDIA_REMOTE_JS: &str = r#"
function backend() {
  var b = $.NSBundle.bundleWithPath('/System/Library/PrivateFrameworks/MediaRemote.framework');
  if (b.isNil() || !b.load) return {error: 'MediaRemote did not load'};
  var R = $.NSClassFromString('MRNowPlayingRequest');
  if (R.isNil()) return {error: 'MRNowPlayingRequest is missing'};
  function str(v) {
    var s = ObjC.deepUnwrap(v);
    return (s === undefined || s === null) ? '' : String(s);
  }
  return {
    snap: function () {
      var p = R.localNowPlayingPlayerPath;
      if (p.isNil() || p.client.isNil()) return null;
      var app = str(p.client.parentApplicationBundleIdentifier) || str(p.client.bundleIdentifier);
      var title = '';
      var it = R.localNowPlayingItem;
      if (!it.isNil() && !it.nowPlayingInfo.isNil()) {
        title = str(it.nowPlayingInfo.objectForKey('kMRMediaRemoteNowPlayingInfoTitle'));
      }
      return {app: app, title: title, state: Number(R.localPlaybackState),
              isPlaying: R.localIsPlaying === true};
    },
    send: function (cmd) {
      ObjC.bindFunction('MRMediaRemoteSendCommand', ['bool', ['int', 'id']]);
      // $() is nil (null would pass NSNull as the options).
      return $.MRMediaRemoteSendCommand(cmd, $()) === true;
    },
    wait: function (seconds) {
      $.NSRunLoop.currentRunLoop.runUntilDate($.NSDate.dateWithTimeIntervalSinceNow(seconds));
    }
  };
}
"#;

/// The decision, shared by every backend: read, then send only to the
/// expected media in the expected state. `state` is MediaRemote's playback
/// state: 1 playing, 2 paused, 3 stopped, 0 unknown.
const DECIDE_JS: &str = r#"
function playing(s) { return s.state === 1 || (s.state === 0 && s.isPlaying); }
function resumable(s) { return s.state === 2 && !s.isPlaying; }
// Not `run`: osascript calls a top-level `run` by itself.
function decide(mode, want) {
  var b;
  try { b = backend(); } catch (e) { return {ok: false, why: String(e)}; }
  if (b.error) return {ok: false, why: b.error};
  var out = {ok: true, now: null, tried: false, sent: false};
  try {
    out.now = b.snap();
    var now = out.now;
    if (mode === 'read' || !now || now.app !== want.app || now.title !== want.title) return out;
    var pause = mode === 'pause';
    if (pause ? !playing(now) : !resumable(now)) return out;
    out.tried = true;
    out.sent = b.send(pause ? 1 : 0);
  } catch (e) {
    out.why = String(e);
    return out;
  }
  if (!out.sent) return out;
  try {
    for (var i = 0; i < 6; i++) {
      b.wait(0.05);
      var after = b.snap();
      out.after = after;
      if (after && (pause ? !playing(after) : playing(after))) break;
    }
  } catch (e) {}
  return out;
}
"#;

/// The now-playing media as a script read it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Snapshot {
    /// The app's bundle id (a browser's, not its media helper's).
    pub app: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub state: i64,
    #[serde(default, rename = "isPlaying")]
    pub is_playing: bool,
}

impl Snapshot {
    pub fn playing(&self) -> bool {
        self.state == 1 || (self.state == 0 && self.is_playing)
    }

    fn same(&self, other: &Snapshot) -> bool {
        self.app == other.app && self.title == other.title
    }
}

/// What a script answers.
#[derive(Debug, Default, Deserialize)]
struct Answer {
    ok: bool,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    now: Option<Snapshot>,
    /// A command was due and sent (`sent`: MediaRemote took it).
    #[serde(default)]
    tried: bool,
    #[serde(default)]
    sent: bool,
}

/// What a script is asked to do.
#[derive(Clone, Copy)]
enum Mode {
    Read,
    Pause,
    Play,
}

struct Inner {
    run: Runner,
    backend: String,
}

impl Inner {
    fn ask(&self, mode: Mode, want: Option<&Snapshot>) -> PResult<Answer> {
        let mode = match mode {
            Mode::Read => "read",
            Mode::Pause => "pause",
            Mode::Play => "play",
        };
        // JSON strings are JavaScript string literals: a title is data.
        let want = match want {
            Some(w) => serde_json::json!({"app": w.app, "title": w.title}),
            None => serde_json::json!({"app": "", "title": ""}),
        };
        let script = format!(
            "ObjC.import('Foundation');\n{}\n{DECIDE_JS}\nJSON.stringify(decide('{mode}', {want}));",
            self.backend
        );
        let out = (self.run)(&script)?;
        serde_json::from_str(&out).map_err(|e| format!("osascript answered {out:?}: {e}"))
    }
}

/// The Now Playing app, as media sessions (none, or the one now playing).
pub struct NowPlayingMedia {
    inner: Arc<Inner>,
    /// Apps left to another path (`MEDIA_APPS`).
    skip: Vec<String>,
}

impl Default for NowPlayingMedia {
    fn default() -> Self {
        Self::new()
    }
}

impl NowPlayingMedia {
    /// The real one: MediaRemote through `osascript`, Music and Spotify
    /// left to AppleScript.
    pub fn new() -> Self {
        Self::with(
            Arc::new(|script: &str| osascript(script, true, CALL_TIMEOUT)),
            MEDIA_REMOTE_JS,
            MEDIA_APPS,
        )
    }

    /// A seam: `run` runs the scripts, `backend` is the JavaScript that
    /// defines `backend()`, `skip` the apps to leave alone.
    pub fn with(run: Runner, backend: &str, skip: &[&str]) -> Self {
        NowPlayingMedia {
            inner: Arc::new(Inner {
                run,
                backend: backend.to_string(),
            }),
            skip: skip.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The now-playing media, `None` when nothing is. Fails when MediaRemote
    /// cannot be used (or the call timed out).
    pub fn read(&self) -> PResult<Option<Snapshot>> {
        let a = self.inner.ask(Mode::Read, None)?;
        if !a.ok {
            return Err(a.why.unwrap_or_else(|| "MediaRemote is unavailable".into()));
        }
        Ok(a.now)
    }

    /// The session for a read (`read`'s result): one for media of an app
    /// not skipped, else none. A failed read is no session (the reason is
    /// reported once on stderr).
    pub fn sessions_from(&self, read: PResult<Option<Snapshot>>) -> Vec<Box<dyn MediaSession>> {
        let now = match read {
            Ok(Some(now)) => now,
            Ok(None) => return Vec::new(),
            Err(e) => {
                static SAID: AtomicBool = AtomicBool::new(false);
                if !SAID.swap(true, Ordering::SeqCst) {
                    eprintln!("sonara-system: [pause] Now Playing is unavailable: {e}");
                }
                return Vec::new();
            }
        };
        if now.app.is_empty() || self.skip.contains(&now.app) {
            return Vec::new();
        }
        vec![Box::new(NowPlayingSession {
            inner: self.inner.clone(),
            fresh: Cell::new(Some(now.playing())),
            want: now,
        })]
    }

    pub fn sessions(&self) -> Vec<Box<dyn MediaSession>> {
        self.sessions_from(self.read())
    }
}

/// One now-playing title of one app.
struct NowPlayingSession {
    inner: Arc<Inner>,
    want: Snapshot,
    /// Whether it played when listed, for the first `is_playing` (the
    /// listing is milliseconds old; `pause` checks again anyway).
    fresh: Cell<Option<bool>>,
}

impl NowPlayingSession {
    fn gone(&self) -> String {
        format!("{} is no longer the Now Playing media", self.want.app)
    }
}

impl MediaSession for NowPlayingSession {
    fn app_id(&self) -> PResult<String> {
        Ok(self.want.app.clone())
    }

    /// `Err` means other media (or none) is now playing: there is nothing
    /// to resume. A read that failed or timed out is "not playing", so a
    /// resume still tries `play`, which checks again.
    fn is_playing(&self) -> PResult<bool> {
        if let Some(p) = self.fresh.take() {
            return Ok(p);
        }
        match self.inner.ask(Mode::Read, None) {
            Ok(a) if a.ok => match a.now {
                Some(now) if now.same(&self.want) => Ok(now.playing()),
                _ => Err(self.gone()),
            },
            _ => Ok(false),
        }
    }

    /// Pause it if it is still the now-playing media and plays. Anything
    /// else is an error, so the pauser never resumes it later; a timeout
    /// passes through (it may have paused).
    fn pause(&self) -> PResult<()> {
        self.fresh.set(None);
        let a = self.inner.ask(Mode::Pause, Some(&self.want))?;
        if !a.ok {
            return Err(a.why.unwrap_or_else(|| "MediaRemote is unavailable".into()));
        }
        if a.sent {
            return Ok(());
        }
        if a.tried {
            return Err(a
                .why
                .unwrap_or_else(|| "MediaRemote did not take the pause".into()));
        }
        match a.now {
            Some(now) if now.same(&self.want) => Err(format!("{} is not playing", self.want.app)),
            _ => Err(self.gone()),
        }
    }

    /// Resume it only if it is still the now-playing media and paused:
    /// other media, or the same one playing or stopped, is left alone (and
    /// is no error: there is nothing to retry). A failure or a timeout is
    /// an error, so the pauser retries it.
    fn play(&self) -> PResult<()> {
        self.fresh.set(None);
        let a = self.inner.ask(Mode::Play, Some(&self.want))?;
        if !a.ok {
            return Err(a.why.unwrap_or_else(|| "MediaRemote is unavailable".into()));
        }
        if a.tried && !a.sent {
            return Err(a
                .why
                .unwrap_or_else(|| "MediaRemote did not take the play".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::TIMED_OUT;
    use std::sync::Mutex;

    /// A runner that answers canned JSON and records each script's mode.
    fn canned(answers: Vec<PResult<&'static str>>) -> (Runner, Arc<Mutex<Vec<String>>>) {
        let modes = Arc::new(Mutex::new(Vec::new()));
        let seen = modes.clone();
        let answers = Mutex::new(answers.into_iter());
        let run: Runner = Arc::new(move |script: &str| {
            let mode = ["read", "pause", "play"]
                .into_iter()
                .find(|m| script.contains(&format!("decide('{m}'")))
                .unwrap_or("?");
            seen.lock().unwrap().push(mode.to_string());
            answers
                .lock()
                .unwrap()
                .next()
                .expect("an answer per call")
                .map(str::to_string)
        });
        (run, modes)
    }

    const CHROME_PLAYING: &str = r#"{"ok":true,"now":{"app":"com.google.Chrome","title":"A video","state":1,"isPlaying":true}}"#;
    const CHROME_PAUSED: &str = r#"{"ok":true,"now":{"app":"com.google.Chrome","title":"A video","state":2,"isPlaying":false}}"#;

    fn chrome(run: Runner) -> Box<dyn MediaSession> {
        let media = NowPlayingMedia::with(run, "", MEDIA_APPS);
        let read = Ok(Some(Snapshot {
            app: "com.google.Chrome".into(),
            title: "A video".into(),
            state: 1,
            is_playing: true,
        }));
        media.sessions_from(read).pop().unwrap()
    }

    #[test]
    fn the_now_playing_app_is_one_session_named_by_its_bundle_id() {
        let (run, _) = canned(vec![Ok(CHROME_PLAYING)]);
        let media = NowPlayingMedia::with(run, "", MEDIA_APPS);
        let s = media.sessions();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].app_id().unwrap(), "com.google.Chrome");
        assert_eq!(s[0].is_playing(), Ok(true), "from the listing");
    }

    #[test]
    fn music_and_spotify_are_left_to_applescript() {
        let (run, _) = canned(vec![Ok(
            r#"{"ok":true,"now":{"app":"com.spotify.client","title":"x","state":1}}"#,
        )]);
        assert!(NowPlayingMedia::with(run, "", MEDIA_APPS)
            .sessions()
            .is_empty());
    }

    #[test]
    fn nothing_now_playing_is_no_session() {
        let (run, _) = canned(vec![Ok(r#"{"ok":true,"now":null}"#)]);
        assert!(NowPlayingMedia::with(run, "", MEDIA_APPS)
            .sessions()
            .is_empty());
    }

    #[test]
    fn unavailable_mediaremote_or_a_timeout_is_no_session_not_an_error() {
        let (run, _) = canned(vec![
            Ok(r#"{"ok":false,"why":"MediaRemote did not load"}"#),
            Err(format!("{TIMED_OUT} after 2000 ms")),
            Ok("not json"),
        ]);
        let media = NowPlayingMedia::with(run, "", MEDIA_APPS);
        assert!(media.sessions().is_empty());
        assert!(media.sessions().is_empty());
        assert!(media.sessions().is_empty());
    }

    #[test]
    fn a_pause_that_mediaremote_took_is_ok() {
        let (run, modes) = canned(vec![Ok(r#"{"ok":true,"tried":true,"sent":true}"#)]);
        assert_eq!(chrome(run).pause(), Ok(()));
        assert_eq!(*modes.lock().unwrap(), ["pause"]);
    }

    #[test]
    fn media_that_stopped_playing_or_changed_is_not_paused_and_not_resumed_later() {
        // Not sent because it is paused already: an error, so the pauser
        // does not record it.
        let (run, _) = canned(vec![Ok(CHROME_PAUSED)]);
        assert!(chrome(run).pause().unwrap_err().contains("not playing"));
        let (run, _) = canned(vec![Ok(
            r#"{"ok":true,"now":{"app":"com.apple.Safari","title":"y","state":1}}"#,
        )]);
        assert!(chrome(run).pause().unwrap_err().contains("no longer"));
        let (run, _) = canned(vec![Ok(r#"{"ok":true,"tried":true,"sent":false}"#)]);
        assert!(chrome(run).pause().is_err());
    }

    #[test]
    fn a_pause_that_timed_out_is_reported_as_a_timeout() {
        let (run, _) = canned(vec![Err(format!("{TIMED_OUT} after 2000 ms"))]);
        let e = chrome(run).pause().unwrap_err();
        assert!(crate::platform::timed_out(&e), "{e}");
    }

    #[test]
    fn resume_reads_again_and_other_media_now_playing_is_gone() {
        let (run, modes) = canned(vec![
            Ok(r#"{"ok":true,"tried":true,"sent":true}"#),
            Ok(
                r#"{"ok":true,"now":{"app":"com.google.Chrome","title":"Another video","state":2}}"#,
            ),
        ]);
        let s = chrome(run);
        s.pause().unwrap();
        assert!(s.is_playing().is_err(), "another title: nothing to resume");
        assert_eq!(*modes.lock().unwrap(), ["pause", "read"]);
    }

    #[test]
    fn a_failed_read_on_resume_still_tries_play() {
        let (run, _) = canned(vec![
            Err(format!("{TIMED_OUT} after 2000 ms")),
            Ok(r#"{"ok":false,"why":"x"}"#),
        ]);
        let s = chrome(run);
        assert_eq!(s.is_playing(), Ok(true), "the listing's state, once");
        assert_eq!(s.is_playing(), Ok(false));
        assert_eq!(s.is_playing(), Ok(false));
    }

    #[test]
    fn play_that_found_nothing_to_resume_is_ok_and_a_failure_is_retried() {
        // The script found other media or the same playing: nothing sent,
        // nothing to retry.
        let (run, _) = canned(vec![Ok(CHROME_PLAYING)]);
        assert_eq!(chrome(run).play(), Ok(()));
        let (run, _) = canned(vec![Ok(r#"{"ok":true,"tried":true,"sent":false}"#)]);
        assert!(chrome(run).play().is_err());
        let (run, _) = canned(vec![Ok(r#"{"ok":false,"why":"MediaRemote did not load"}"#)]);
        assert!(chrome(run).play().is_err());
        let (run, _) = canned(vec![Err(format!("{TIMED_OUT} after 2000 ms"))]);
        assert!(chrome(run).play().is_err());
    }

    #[test]
    fn a_title_cannot_break_out_of_the_script() {
        let seen = Arc::new(Mutex::new(String::new()));
        let keep = seen.clone();
        let run: Runner = Arc::new(move |script: &str| {
            *keep.lock().unwrap() = script.to_string();
            Ok(r#"{"ok":true}"#.into())
        });
        let media = NowPlayingMedia::with(run, "", MEDIA_APPS);
        let s = media
            .sessions_from(Ok(Some(Snapshot {
                app: "org.videolan.vlc".into(),
                title: "a'); evil(); ('\"\n".into(),
                state: 1,
                is_playing: true,
            })))
            .pop()
            .unwrap();
        let _ = s.pause();
        let script = seen.lock().unwrap().clone();
        assert!(
            script.contains(r#""title":"a'); evil(); ('\"\n""#),
            "{script}"
        );
    }
}
