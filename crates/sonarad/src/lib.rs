//! `sonarad`: the Sonara host process. It runs one L1 reader
//! (`sonara_reader::ReaderHandle`) and serves protocol v1 core
//! (`docs/protocol-v1.md`) to clients on this PC:
//!
//! - TCP JSON lines and HTTP (`POST /v1/<type>`, SSE `GET /v1/events`), both
//!   bound to 127.0.0.1 on ephemeral ports, never to another address.
//! - Discovery through `runtime.json` in the home folder, one instance per
//!   user and home (a named mutex), idle exit and idle-only takeover.
//! - The `channels` extension (L2, `sonara_channels`), the `agent`
//!   extension (L3, `sonara_agent`, which needs `channels`) and the
//!   `system` extension (L4, `sonara_system`: ducking or media pausing,
//!   hotkeys, the settings page) once a client asks for them in `hello`.
//!
//! The protocol logic (`protocol`) is synchronous and transport-free; `tcp`
//! and `http` only frame requests, replies and events.
pub mod agent_ext;
pub mod args;
pub mod channels_ext;
pub mod config;
pub mod cues;
pub mod engines;
pub mod engines_ext;
pub mod events;
pub mod home;
pub mod http;
pub mod instance;
pub mod lifetime;
pub mod migrate;
pub mod null_output;
pub mod protocol;
pub mod quiet;
pub mod runtime_file;
pub mod settings_page;
pub mod support_log;
pub mod system_ext;
pub mod tcp;
pub mod trace_log;
pub mod wav_output;
pub mod wire;

/// The runtime version (the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
