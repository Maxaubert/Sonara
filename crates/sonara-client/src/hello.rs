//! The `hello` every connection starts with.
use serde_json::{json, Value};

/// The extensions of the Claude product: `agent` and `system` (hotkeys,
/// ducking, the settings page, spoken cues).
pub const PRODUCT: &[&str] = &["agent", "system"];

/// Who a client is and what it asks for in its `hello`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hello<'a> {
    /// `client.name`.
    pub name: &'a str,
    /// `client.version`.
    pub version: &'a str,
    pub extensions: &'a [&'a str],
    /// Keep the runtime up after the connection closes.
    pub keep_alive: bool,
}

impl Hello<'_> {
    /// The `hello` message with the runtime's `token`.
    pub fn message(&self, token: &str) -> Value {
        json!({
            "type": "hello",
            "token": token,
            "client": {"name": self.name, "version": self.version},
            "extensions": self.extensions,
            "keep_alive": self.keep_alive,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_carries_the_token_the_client_and_the_extensions() {
        let h = Hello {
            name: "x",
            version: "1.2.3",
            extensions: PRODUCT,
            keep_alive: true,
        }
        .message("tok");
        assert_eq!(h["type"], "hello");
        assert_eq!(h["token"], "tok");
        assert_eq!(h["client"], json!({"name": "x", "version": "1.2.3"}));
        assert_eq!(h["extensions"], json!(["agent", "system"]));
        assert_eq!(h["keep_alive"], true);
    }
}
