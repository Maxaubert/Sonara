//! `sonara-system`: Sonara L4 (spec section 2 and 6), the Windows extras a
//! host turns on only when a client enables the `system` extension.
//!
//! - [`audio::AudioControl`]: duck other apps' audio sessions
//!   ([`ducking::Ducker`], per-session volume across every render device)
//!   or pause their media ([`pausing::MediaPauser`], GSMTC) while L1 reads,
//!   driven by the reader's state events, with crash-restore files in the
//!   home and a startup sweep. Never leaves other apps ducked or paused.
//! - [`hotkeys::Hotkeys`]: global hotkeys (RegisterHotKey on a message-loop
//!   thread) that hand [`keymap::Action`]s to the host, which maps them to
//!   core, channel and agent controls.
//! - [`keymap`]: actions, the default chords Ctrl+Alt+Up/Down/M/P,
//!   `keymap.json` with bind, unbind and reset, and the AltGr check.
//!
//! Everything Windows-facing goes through [`platform::Platform`]; tests use
//! [`fake::Fake`]. L4 builds only on the L1 facade (R7, `tests/layering.rs`).
pub mod audio;
pub mod ducking;
pub mod fake;
pub mod hotkeys;
pub mod keymap;
pub mod pausing;
pub mod platform;
pub mod state_file;
#[cfg(windows)]
pub mod win;

pub use audio::{Activity, AudioConfig, AudioControl, AudioMode, Status};
pub use hotkeys::{Collision, Hotkeys};
pub use keymap::{Action, Binding, Keymap, Resolved};
pub use platform::Platform;
