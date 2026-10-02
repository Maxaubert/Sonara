//! `sonarad`: the Sonara host process. It runs one L1 reader
//! (`sonara_reader::ReaderHandle`) and serves protocol v1 core
//! (`docs/protocol-v1.md`) to clients on this PC:
//!
//! - TCP JSON lines and HTTP (`POST /v1/<type>`, SSE `GET /v1/events`), both
//!   bound to 127.0.0.1 on ephemeral ports, never to another address.
//! - Discovery through `runtime.json` in the home folder, one instance per
//!   user and home (a named mutex), idle exit and idle-only takeover.
//!
//! The protocol logic (`protocol`) is synchronous and transport-free; `tcp`
//! and `http` only frame requests, replies and events.
pub mod args;
pub mod events;
pub mod home;
pub mod http;
pub mod instance;
pub mod lifetime;
pub mod null_output;
pub mod protocol;
pub mod runtime_file;
pub mod tcp;
pub mod wire;

/// The runtime version (the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
