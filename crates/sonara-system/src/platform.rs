//! The seams to the operating system. Everything Windows-facing (audio
//! sessions, media sessions, hotkey registration, the keyboard layout) is
//! reached only through these traits, so the rules are tested with fakes
//! (`crate::fake`) and never touch the user's real audio, media players or
//! hotkeys. `crate::win::platform()` is the real one.
use std::sync::Arc;

/// Platform calls fail with a message (logged, never raised to speech).
pub type PResult<T> = std::result::Result<T, String>;

/// One app's audio session on one render device.
pub trait AudioSession {
    fn pid(&self) -> u32;
    /// The process image name (`vlc.exe`), empty when unknown.
    fn name(&self) -> String;
    /// Master volume, 0.0 to 1.0.
    fn volume(&self) -> PResult<f32>;
    fn set_volume(&self, level: f32) -> PResult<()>;
}

/// The live audio sessions across every active render device.
pub trait AudioSessions {
    fn sessions(&self) -> PResult<Vec<Box<dyn AudioSession>>>;
}

/// One app's media transport controls (GSMTC).
pub trait MediaSession {
    fn app_id(&self) -> PResult<String>;
    fn is_playing(&self) -> PResult<bool>;
    fn pause(&self) -> PResult<()>;
    fn play(&self) -> PResult<()>;
}

pub trait MediaSessions {
    fn sessions(&self) -> PResult<Vec<Box<dyn MediaSession>>>;
}

/// Global hotkeys of one thread: `register` and `wait` run on the thread
/// that created the registrar (RegisterHotKey and the message loop must
/// share it).
pub trait Registrar {
    /// Register one chord; the error is the OS error code (1409: another
    /// program owns it).
    fn register(&mut self, id: i32, modifiers: u32, vk: u32) -> Result<(), u32>;
    fn unregister(&mut self, id: i32);
    /// Block for the next hotkey press; `None` once stopped.
    fn wait(&mut self) -> Option<i32>;
    /// A handle that ends `wait` from another thread.
    fn stopper(&self) -> Box<dyn Fn() + Send>;
}

/// The keyboard layout, for the AltGr check (E16, #160).
pub trait KeyboardLayout: Send + Sync {
    /// The character AltGr (+ Shift) + `vk` types, if any. AltGr arrives as
    /// LCtrl+RAlt, so a Ctrl+Alt hotkey on that key eats this character.
    fn altgr_char(&self, vk: u32, shift: bool) -> Option<String>;
}

type Factory<T> = Arc<dyn Fn() -> Box<T> + Send + Sync>;

/// A platform: factories, because audio and media objects live on the
/// thread that created them (COM), and a registrar on its pump thread.
#[derive(Clone)]
pub struct Platform {
    /// `windows` or `fake`.
    pub name: &'static str,
    pub audio: Factory<dyn AudioSessions>,
    pub media: Factory<dyn MediaSessions>,
    pub registrar: Factory<dyn Registrar>,
    pub layout: Arc<dyn KeyboardLayout>,
}
