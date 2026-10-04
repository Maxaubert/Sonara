//! Failures of an external engine (spec 8.1): a `Reason`, the HTTP status
//! when there was one, and a message that is already clipped and masked. An
//! `ExtError` never holds request headers, so it cannot carry a key.
pub use crate::Reason;
use std::time::Duration;

/// Provider messages are clipped to this many characters.
pub const MESSAGE_MAX: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtError {
    pub reason: Reason,
    pub status: Option<u16>,
    pub message: String,
    /// A `Retry-After` the provider sent (429, 503).
    pub retry_after: Option<Duration>,
    /// A request parameter the provider's own error body refused (set by
    /// `map_error`, read by `Adapter::adapt`; never from the message, which
    /// also holds the user's label).
    pub refused_param: Option<&'static str>,
}

impl ExtError {
    pub fn new(reason: Reason, message: impl Into<String>) -> ExtError {
        ExtError {
            reason,
            status: None,
            message: clean(&message.into()),
            retry_after: None,
            refused_param: None,
        }
    }

    pub fn with_status(mut self, status: u16) -> ExtError {
        self.status = Some(status);
        self
    }

    pub fn into_engine_error(self) -> crate::Error {
        crate::Error::External {
            reason: self.reason,
            message: self.message,
        }
    }
}

impl std::fmt::Display for ExtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Clipped to `MESSAGE_MAX` characters, credential-looking words masked,
/// on one line.
pub fn clean(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one_line = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let masked = sonara_log::mask(&one_line);
    let mut out: String = masked.chars().take(MESSAGE_MAX).collect();
    if masked.chars().count() > MESSAGE_MAX {
        out.push_str("...");
    }
    out
}

/// The headline of a failure for a person: "OpenAI refused the key".
pub fn headline(reason: Reason, label: &str, host: &str) -> String {
    let host = if host.is_empty() { label } else { host };
    match reason {
        Reason::NoKey => format!("{label} has no key"),
        Reason::Auth => format!("{label} refused the key"),
        Reason::Quota => format!("{label} is out of credit"),
        Reason::RateLimited => format!("{host} is busy"),
        Reason::Network => format!("cannot reach {host}"),
        Reason::Timeout => format!("{host} did not answer in time"),
        Reason::Server => format!("{label} has a server problem"),
        Reason::BadVoice => format!("{label} does not know this voice"),
        Reason::BadConfig => format!("{label} settings do not work"),
        Reason::Format => format!("{label} sent audio Sonara cannot play"),
    }
}

/// The spoken cue of a fallback (spec 8.2).
pub fn cue_text(reason: Reason, label: &str) -> String {
    let what = match reason {
        Reason::NoKey => "has no key",
        Reason::Auth => "refused the key",
        Reason::Quota => "is out of credit",
        Reason::RateLimited => "is busy",
        Reason::Network | Reason::Timeout => "cannot be reached",
        Reason::Server => "has a server problem",
        Reason::BadVoice => "does not know this voice",
        Reason::BadConfig | Reason::Format => "settings do not work",
    };
    format!("{label} {what}. Reading with the built-in voice.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_follow_the_table() {
        assert_eq!(
            cue_text(Reason::NoKey, "OpenAI"),
            "OpenAI has no key. Reading with the built-in voice."
        );
        assert_eq!(
            cue_text(Reason::Timeout, "OpenAI"),
            "OpenAI cannot be reached. Reading with the built-in voice."
        );
        assert_eq!(
            cue_text(Reason::Format, "X"),
            "X settings do not work. Reading with the built-in voice."
        );
        for r in Reason::ALL {
            assert!(cue_text(r, "L").ends_with("Reading with the built-in voice."));
        }
    }

    #[test]
    fn messages_are_clipped_masked_and_on_one_line() {
        let m = clean("Incorrect API key provided: sk-proj-abcdefghijklmnop1234.\nSee docs");
        assert_eq!(m, "Incorrect API key provided: [redacted] See docs");
        let long = clean(&"x".repeat(400));
        assert_eq!(long.chars().count(), MESSAGE_MAX + 3);
        assert_eq!(
            clean("got Authorization: Bearer abc.def"),
            "got Authorization: Bearer [redacted]"
        );
    }
}
