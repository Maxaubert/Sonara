//! Sonara L5 support: the protocol v1 client of a local `sonarad`, shared
//! by the hook (`sonara-hook`), the command line (`sonara-cli`) and, for
//! the home and the file names in it, `sonarad` itself (#255). A leaf
//! crate: it speaks the protocol only and links no runtime crate (R7).
//!
//! - `home`: where the home is (`SONARA_HOME`, else
//!   `%LOCALAPPDATA%\Sonara`), the names of `runtime.json` and of the stop
//!   sentinel `stopped`.
//! - `runtime`: `runtime.json` read back, a connection to the runtime it
//!   names, and the runtime started detached (`sonarad.exe` next to the
//!   caller, extra arguments from `SONARA_RUNTIME_ARGS`).
//! - `hello`: the `hello` a client sends first (`Hello`).
//! - `batch`: fire and forget, as a hook needs it: `hello` and a batch of
//!   messages on one connection (`send`), the runtime started when none
//!   answers, all inside a deadline (`deliver`).
//! - `conn`: request and reply, as the CLI needs it: messages matched to
//!   their replies by `id`, events kept for `Conn::event` (`attach`).
pub mod batch;
pub mod conn;
pub mod hello;
pub mod home;
pub mod runtime;

pub use batch::{deliver, send, send_on, worth_trying, Delivery};
pub use conn::{attach, Conn, TIMEOUT};
pub use hello::{Hello, PRODUCT};
pub use home::{default_home, home, home_from, stopped, RUNTIME_FILE, STOPPED};
pub use runtime::{
    connect, read_runtime, runtime_args, runtime_exe, start_runtime, Runtime, PROBE, RUNTIME_EXE,
};
