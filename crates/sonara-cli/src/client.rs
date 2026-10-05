//! The CLI's side of the protocol v1 client (`sonara_client::conn`): who
//! it says it is in `hello`.
use serde_json::Value;
use sonara_client::Runtime;
pub use sonara_client::{Conn, Hello, PRODUCT, TIMEOUT};
use std::path::Path;

/// The CLI's `hello`.
pub fn hello<'a>(extensions: &'a [&'a str], keep_alive: bool) -> Hello<'a> {
    Hello {
        name: "sonara-cli",
        version: crate::VERSION,
        extensions,
        keep_alive,
    }
}

/// A connection to the runtime of `home` after a successful `hello`, the
/// runtime file's values and the `hello` reply; `None` when no runtime
/// answers.
pub fn attach(
    home: &Path,
    extensions: &[&str],
    keep_alive: bool,
) -> Option<(Runtime, Conn, Value)> {
    sonara_client::attach(home, &hello(extensions, keep_alive))
}
