//! `sonara.exe`, the command line of the Claude Code plugin's runtime
//! (#202). It ships in the runtime zip next to `sonarad.exe` and
//! `sonara-hook.exe` and, like the hook, speaks protocol v1 only (no
//! runtime crate). The plugin's slash commands call it through
//! `bin/sonara`, which installs the runtime first when it is missing.
//!
//! - `start`: clear the stop sentinel (`<home>\stopped`) and make sure
//!   this release's `sonarad` runs, standalone and armed (`hello` with
//!   `agent`, `system` and `keep_alive`, like the hook). A runtime of
//!   another release (an upgrade) is shut down and replaced, and older
//!   folders in `%LOCALAPPDATA%\Sonara\runtime\` are removed.
//! - `stop`: write the stop sentinel (the hooks then never start the
//!   runtime) and ask the runtime to exit (`shutdown`, extension `system`),
//!   which restores other apps first.
//! - `settings`: `start`, then open the settings page (its URL carries the
//!   token) in the default browser.
//! - `doctor`: one row per check; informational rows never fail.
//! - `uninstall [--keep LIST]`: stop, remove the runtime folder and the
//!   home except what the user keeps, and leave the stop sentinel so the
//!   plugin's hooks stay quiet until `/sonara:start`.
//!
//! Paths: the home is `SONARA_HOME`, else `%LOCALAPPDATA%\Sonara` (as
//! `sonarad`); the runtime folders are `%LOCALAPPDATA%\Sonara\runtime\<version>\`.
pub mod client;
pub mod doctor;
pub mod lifecycle;
pub mod paths;
pub mod uninstall;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
