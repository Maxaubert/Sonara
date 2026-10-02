//! A fake platform for tests: audio sessions, media sessions, a hotkey
//! registrar and a keyboard layout, held in memory or in a JSON file.
//!
//! The file form (`sonarad --system fake`, a testing aid) lets a black-box
//! test set up the "apps", kill the runtime and check what a new runtime's
//! startup sweep restores, across processes. Every operation reads the
//! file, changes it and writes it back (atomically), so the test can edit
//! it between calls.
//!
//! ```json
//! {"audio": [{"pid": 100, "name": "vlc.exe", "volume": 0.8}],
//!  "media": [{"app": "spotify", "playing": true}],
//!  "taken": [{"mods": 3, "vk": 38}],
//!  "altgr": [{"vk": 77, "shift": false, "char": "µ"}],
//!  "hotkeys": [{"id": 1, "mods": 16387, "vk": 38}]}
//! ```
//!
//! `hotkeys` is written by the fake: the chords registered now.
use crate::platform::{
    AudioSession, AudioSessions, KeyboardLayout, MediaSession, MediaSessions, PResult, Platform,
    Registrar,
};
use crate::state_file;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FakeAudio {
    pub pid: u32,
    #[serde(default)]
    pub name: String,
    pub volume: f32,
    /// Every volume read and write fails (an invalidated device).
    #[serde(default, skip_serializing_if = "is_false")]
    pub broken: bool,
    /// The n-th `set_volume` (1-based) fails once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_set_on: Option<u32>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub set_calls: u32,
    /// A stable identity, so a session object stays bound to its app while
    /// the list changes.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub uid: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FakeMedia {
    pub app: String,
    pub playing: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub fail_play: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub plays: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pauses: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FakeChord {
    #[serde(default)]
    pub id: i32,
    pub mods: u32,
    pub vk: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FakeAltGr {
    pub vk: u32,
    #[serde(default)]
    pub shift: bool,
    pub char: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Everything the fake platform holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct World {
    #[serde(default)]
    pub audio: Vec<FakeAudio>,
    #[serde(default)]
    pub media: Vec<FakeMedia>,
    /// Listing audio sessions fails.
    #[serde(default, skip_serializing_if = "is_false")]
    pub audio_down: bool,
    /// Listing media sessions fails.
    #[serde(default, skip_serializing_if = "is_false")]
    pub media_down: bool,
    /// Chords another program owns (mods without MOD_NOREPEAT).
    #[serde(default)]
    pub taken: Vec<FakeChord>,
    #[serde(default)]
    pub altgr: Vec<FakeAltGr>,
    /// Registered now (mods as given to RegisterHotKey).
    #[serde(default)]
    pub hotkeys: Vec<FakeChord>,
}

enum Press {
    Hotkey(i32),
    Quit,
}

struct Inner {
    world: World,
    file: Option<PathBuf>,
    next_uid: u64,
    /// The running registrar's queue (one at a time, like one pump thread).
    presses: Option<Sender<Press>>,
    /// Called before every `set_volume` (tests that check ordering).
    on_set: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Inner {
    fn load(&mut self) {
        if let Some(f) = &self.file {
            if let Some(w) = state_file::read::<World>(f) {
                self.world = w;
            }
        }
        for a in self.world.audio.iter_mut() {
            if a.uid == 0 {
                self.next_uid += 1;
                a.uid = self.next_uid;
            }
            self.next_uid = self.next_uid.max(a.uid);
        }
    }

    fn save(&self) {
        if let Some(f) = &self.file {
            let _ = state_file::write(f, &self.world);
        }
    }
}

/// The fake platform's shared world; clones share it.
#[derive(Clone)]
pub struct Fake {
    inner: Arc<Mutex<Inner>>,
}

impl Default for Fake {
    fn default() -> Self {
        Fake::new()
    }
}

impl Fake {
    /// An empty world in memory.
    pub fn new() -> Fake {
        Fake {
            inner: Arc::new(Mutex::new(Inner {
                world: World::default(),
                file: None,
                next_uid: 0,
                presses: None,
                on_set: None,
            })),
        }
    }

    /// A world kept in `path` (read and written on every operation).
    pub fn file(path: PathBuf) -> Fake {
        let f = Fake::new();
        f.inner.lock().unwrap().file = Some(path);
        f
    }

    fn with<R>(&self, f: impl FnOnce(&mut World) -> R) -> R {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.load();
        let r = f(&mut inner.world);
        inner.save();
        r
    }

    /// Change the world (tests).
    pub fn edit(&self, f: impl FnOnce(&mut World)) {
        self.with(f)
    }

    /// A copy of the world now.
    pub fn world(&self) -> World {
        self.with(|w| w.clone())
    }

    /// Add an app's audio session.
    pub fn add_audio(&self, pid: u32, name: &str, volume: f32) {
        self.with(|w| {
            w.audio.push(FakeAudio {
                pid,
                name: name.into(),
                volume,
                ..Default::default()
            })
        });
    }

    /// The volume of the first session of `pid`.
    pub fn volume(&self, pid: u32) -> Option<f32> {
        self.with(|w| w.audio.iter().find(|a| a.pid == pid).map(|a| a.volume))
    }

    pub fn add_media(&self, app: &str, playing: bool) {
        self.with(|w| {
            w.media.push(FakeMedia {
                app: app.into(),
                playing,
                ..Default::default()
            })
        });
    }

    pub fn media(&self, app: &str) -> Option<FakeMedia> {
        self.with(|w| w.media.iter().find(|m| m.app == app).cloned())
    }

    /// Run `f` before every `set_volume` from now on.
    pub fn on_set_volume(&self, f: impl Fn() + Send + Sync + 'static) {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).on_set = Some(Arc::new(f));
    }

    /// Press a registered hotkey (as the OS would post WM_HOTKEY).
    pub fn press(&self, id: i32) {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tx) = &inner.presses {
            let _ = tx.send(Press::Hotkey(id));
        }
    }

    /// The platform made of this world.
    pub fn platform(&self) -> Platform {
        let a = self.clone();
        let m = self.clone();
        let r = self.clone();
        Platform {
            name: "fake",
            audio: Arc::new(move || {
                Box::new(FakeAudioSessions(a.clone())) as Box<dyn AudioSessions>
            }),
            media: Arc::new(move || {
                Box::new(FakeMediaSessions(m.clone())) as Box<dyn MediaSessions>
            }),
            registrar: Arc::new(move || {
                let (tx, rx) = mpsc::channel();
                r.inner.lock().unwrap_or_else(|p| p.into_inner()).presses = Some(tx.clone());
                Box::new(FakeRegistrar {
                    fake: r.clone(),
                    rx,
                    tx,
                }) as Box<dyn Registrar>
            }),
            layout: Arc::new(FakeLayout(self.clone())),
        }
    }
}

struct FakeAudioSessions(Fake);

impl AudioSessions for FakeAudioSessions {
    fn sessions(&self) -> PResult<Vec<Box<dyn AudioSession>>> {
        self.0.with(|w| {
            if w.audio_down {
                return Err("audio sessions unavailable".to_string());
            }
            Ok(w.audio
                .iter()
                .map(|a| {
                    Box::new(FakeAudioSession {
                        fake: self.0.clone(),
                        uid: a.uid,
                        pid: a.pid,
                        name: a.name.clone(),
                    }) as Box<dyn AudioSession>
                })
                .collect())
        })
    }
}

struct FakeAudioSession {
    fake: Fake,
    uid: u64,
    pid: u32,
    name: String,
}

impl AudioSession for FakeAudioSession {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn volume(&self) -> PResult<f32> {
        self.fake
            .with(|w| match w.audio.iter().find(|a| a.uid == self.uid) {
                Some(a) if !a.broken => Ok(a.volume),
                Some(_) => Err("AUDCLNT_E_DEVICE_INVALIDATED".into()),
                None => Err("session gone".into()),
            })
    }

    fn set_volume(&self, level: f32) -> PResult<()> {
        let hook = self
            .fake
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .on_set
            .clone();
        if let Some(h) = hook {
            h();
        }
        self.fake
            .with(|w| match w.audio.iter_mut().find(|a| a.uid == self.uid) {
                Some(a) => {
                    a.set_calls += 1;
                    if a.broken {
                        return Err("AUDCLNT_E_DEVICE_INVALIDATED".into());
                    }
                    if a.fail_set_on == Some(a.set_calls) {
                        return Err("transient".into());
                    }
                    a.volume = level;
                    Ok(())
                }
                None => Err("session gone".into()),
            })
    }
}

struct FakeMediaSessions(Fake);

impl MediaSessions for FakeMediaSessions {
    fn sessions(&self) -> PResult<Vec<Box<dyn MediaSession>>> {
        self.0.with(|w| {
            if w.media_down {
                return Err("media sessions unavailable".to_string());
            }
            Ok((0..w.media.len())
                .map(|i| {
                    Box::new(FakeMediaSession {
                        fake: self.0.clone(),
                        app: w.media[i].app.clone(),
                        index: i,
                    }) as Box<dyn MediaSession>
                })
                .collect())
        })
    }
}

struct FakeMediaSession {
    fake: Fake,
    app: String,
    index: usize,
}

impl FakeMediaSession {
    fn with<R>(&self, f: impl FnOnce(&mut FakeMedia) -> PResult<R>) -> PResult<R> {
        self.fake.with(|w| match w.media.get_mut(self.index) {
            Some(m) if m.app == self.app => f(m),
            _ => Err("session gone".into()),
        })
    }
}

impl MediaSession for FakeMediaSession {
    fn app_id(&self) -> PResult<String> {
        Ok(self.app.clone())
    }

    fn is_playing(&self) -> PResult<bool> {
        self.with(|m| Ok(m.playing))
    }

    fn pause(&self) -> PResult<()> {
        self.with(|m| {
            m.pauses += 1;
            m.playing = false;
            Ok(())
        })
    }

    fn play(&self) -> PResult<()> {
        self.with(|m| {
            m.plays += 1;
            if m.fail_play {
                return Err("busy".into());
            }
            m.playing = true;
            Ok(())
        })
    }
}

struct FakeRegistrar {
    fake: Fake,
    rx: Receiver<Press>,
    tx: Sender<Press>,
}

impl Registrar for FakeRegistrar {
    fn register(&mut self, id: i32, modifiers: u32, vk: u32) -> Result<(), u32> {
        self.fake.with(|w| {
            let plain = modifiers & !crate::keymap::MOD_NOREPEAT;
            let clash = w.taken.iter().any(|t| t.mods == plain && t.vk == vk)
                || w.hotkeys
                    .iter()
                    .any(|h| h.mods & !crate::keymap::MOD_NOREPEAT == plain && h.vk == vk);
            if clash {
                return Err(crate::hotkeys::ERROR_HOTKEY_ALREADY_REGISTERED);
            }
            w.hotkeys.push(FakeChord {
                id,
                mods: modifiers,
                vk,
            });
            Ok(())
        })
    }

    fn unregister(&mut self, id: i32) {
        self.fake.with(|w| w.hotkeys.retain(|h| h.id != id));
    }

    fn wait(&mut self) -> Option<i32> {
        match self.rx.recv() {
            Ok(Press::Hotkey(id)) => Some(id),
            _ => None,
        }
    }

    fn stopper(&self) -> Box<dyn Fn() + Send> {
        let tx = self.tx.clone();
        Box::new(move || {
            let _ = tx.send(Press::Quit);
        })
    }
}

struct FakeLayout(Fake);

impl KeyboardLayout for FakeLayout {
    fn altgr_char(&self, vk: u32, shift: bool) -> Option<String> {
        self.0.with(|w| {
            w.altgr
                .iter()
                .find(|a| a.vk == vk && a.shift == shift)
                .map(|a| a.char.clone())
        })
    }
}
