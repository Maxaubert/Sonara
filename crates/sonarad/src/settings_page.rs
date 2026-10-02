//! The settings page (`GET /settings` on the HTTP port, `system`
//! extension). The page is compiled in (`assets/settings.html`) and drives
//! the runtime only through the public HTTP API (`POST /v1/<type>`), with
//! the token injected here.
//!
//! The token never reaches another origin:
//! - the page is served only with the token in the query (the
//!   `settings_url` a client got over the authenticated protocol), so
//!   another local user or a web page cannot fetch it;
//! - the `Host` header must be this loopback port (no DNS rebinding);
//! - the API sends no CORS headers, and the page forbids framing, external
//!   connections and referrers (`Content-Security-Policy`,
//!   `Referrer-Policy: no-referrer`).
use crate::protocol::token_eq;

pub const PAGE: &str = include_str!("../assets/settings.html");

/// The placeholder the token replaces (a JSON string in the page script).
pub const TOKEN_SLOT: &str = "\"__SONARA_TOKEN__\"";

/// Response headers of the page.
pub const HEADERS: &[(&str, &str)] = &[
    ("content-type", "text/html; charset=utf-8"),
    ("cache-control", "no-store"),
    ("referrer-policy", "no-referrer"),
    ("x-content-type-options", "nosniff"),
    ("x-frame-options", "DENY"),
    (
        "content-security-policy",
        "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
         connect-src 'self'; img-src data:; base-uri 'none'; form-action 'none'; \
         frame-ancestors 'none'",
    ),
];

/// One query parameter's value (no percent-decoding needed: the token is
/// hex).
fn query_param<'a>(query: Option<&'a str>, name: &str) -> Option<&'a str> {
    query?
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// The page for a request with this `Host` header and query, or the HTTP
/// status and message to answer instead.
pub fn render(
    host: Option<&str>,
    query: Option<&str>,
    port: u16,
    token: &str,
) -> Result<String, (u16, &'static str)> {
    let allowed = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host_ok = host.is_some_and(|h| allowed.iter().any(|a| a.eq_ignore_ascii_case(h)));
    if !host_ok {
        return Err((403, "open the settings page through its settings_url"));
    }
    match query_param(query, "token") {
        Some(t) if token_eq(t, token) => {}
        _ => return Err((401, "missing or wrong token: open the settings_url")),
    }
    // The token is hex, so it needs no escaping inside a JS string.
    debug_assert!(token.chars().all(|c| c.is_ascii_alphanumeric()));
    Ok(PAGE.replacen(TOKEN_SLOT, &format!("\"{token}\""), 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_needs_the_loopback_host_and_the_token() {
        let ok = render(Some("127.0.0.1:5000"), Some("token=abc123"), 5000, "abc123").unwrap();
        assert!(ok.contains("\"abc123\""));
        assert!(!ok.contains(TOKEN_SLOT));
        assert!(render(
            Some("localhost:5000"),
            Some("x=1&token=abc123"),
            5000,
            "abc123"
        )
        .is_ok());
        assert_eq!(
            render(
                Some("evil.example:5000"),
                Some("token=abc123"),
                5000,
                "abc123"
            )
            .unwrap_err()
            .0,
            403,
            "a DNS-rebound name is refused"
        );
        assert_eq!(
            render(None, Some("token=abc123"), 5000, "abc123")
                .unwrap_err()
                .0,
            403
        );
        assert_eq!(
            render(Some("127.0.0.1:5001"), Some("token=abc123"), 5000, "abc123")
                .unwrap_err()
                .0,
            403
        );
        assert_eq!(
            render(Some("127.0.0.1:5000"), None, 5000, "abc123")
                .unwrap_err()
                .0,
            401
        );
        assert_eq!(
            render(Some("127.0.0.1:5000"), Some("token=nope"), 5000, "abc123")
                .unwrap_err()
                .0,
            401
        );
    }

    #[test]
    fn the_asset_has_exactly_one_token_slot_and_uses_only_the_v1_api() {
        assert_eq!(PAGE.matches(TOKEN_SLOT).count(), 1);
        assert!(
            !PAGE.contains("/api/"),
            "the Python daemon's private API is gone"
        );
        assert!(PAGE.contains("/v1/"));
    }
}
