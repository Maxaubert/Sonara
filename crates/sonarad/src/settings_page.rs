//! The settings page (`GET /settings` on the HTTP port, `system`
//! extension). The page is compiled in (`assets/settings/`) and drives
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
//!
//! The page's fonts (Geist and Geist Mono, SIL OFL 1.1, `assets/fonts`)
//! are compiled in too and inlined as `data:` URLs, so the page loads
//! nothing from another origin (#237).
use std::sync::OnceLock;

use crate::protocol::token_eq;

/// The page, kept in parts (`assets/settings/`: the markup around the style
/// sheet and the script) and joined at compile time.
pub const PAGE: &str = concat!(
    include_str!("../assets/settings/head.html"),
    include_str!("../assets/settings/style.css"),
    include_str!("../assets/settings/body.html"),
    include_str!("../assets/settings/script.js"),
    include_str!("../assets/settings/foot.html"),
);

/// The placeholder the token replaces (a JSON string in the page script).
pub const TOKEN_SLOT: &str = "\"__SONARA_TOKEN__\"";

/// The placeholder in the page's style sheet the `@font-face` rules replace.
pub const FONTS_SLOT: &str = "/*__SONARA_FONTS__*/";

/// The placeholder in the page script `config::page_ranges` replaces: the
/// sliders' and the stepper's ranges and defaults come from `SCHEMA` (#257).
pub const SCHEMA_SLOT: &str = "/*__SONARA_SCHEMA__*/{}";

/// The fonts: (family, woff2 bytes). Latin subsets of the variable fonts
/// (weights 100 to 900); other scripts fall back to the system font.
const FONTS: &[(&str, &[u8])] = &[
    ("Geist", include_bytes!("../assets/fonts/Geist-latin.woff2")),
    (
        "Geist Mono",
        include_bytes!("../assets/fonts/GeistMono-latin.woff2"),
    ),
];

/// Standard base64 with padding (RFC 4648), for the `data:` URLs.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The `@font-face` rules, built once.
fn font_css() -> &'static str {
    static CSS: OnceLock<String> = OnceLock::new();
    CSS.get_or_init(|| {
        FONTS
            .iter()
            .map(|(family, woff2)| {
                format!(
                    "@font-face{{font-family:\"{family}\";font-style:normal;font-weight:100 900;font-display:swap;src:url(data:font/woff2;base64,{}) format(\"woff2\")}}",
                    base64(woff2)
                )
            })
            .collect()
    })
}

/// The page with its fonts and the schema's ranges in place, built once
/// (the token goes in per request).
fn page_with_fonts() -> &'static str {
    static FULL: OnceLock<String> = OnceLock::new();
    FULL.get_or_init(|| {
        PAGE.replacen(FONTS_SLOT, font_css(), 1).replacen(
            SCHEMA_SLOT,
            &crate::config::page_ranges().to_string(),
            1,
        )
    })
}

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
         connect-src 'self'; img-src data:; font-src data:; base-uri 'none'; \
         form-action 'none'; frame-ancestors 'none'",
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
    Ok(page_with_fonts().replacen(TOKEN_SLOT, &format!("\"{token}\""), 1))
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

    #[test]
    fn the_page_takes_ranges_and_defaults_from_the_schema() {
        assert_eq!(PAGE.matches(SCHEMA_SLOT).count(), 1);
        let page = render(Some("127.0.0.1:5000"), Some("token=abc123"), 5000, "abc123").unwrap();
        assert!(!page.contains(SCHEMA_SLOT));
        let ranges = crate::config::page_ranges();
        assert!(page.contains(&format!("const SCHEMA = {ranges};")));
        assert_eq!(
            ranges["rate"],
            serde_json::json!({"min": 100, "max": 400, "default": 250}),
            "the rate default is SCHEMA's 250, not a page constant (#257)"
        );
        for key in ["volume", "duck_level", "minqueue"] {
            assert_eq!(ranges[key]["default"], crate::config::default(key).unwrap());
        }
        // The page hard-codes no range or default of its own.
        for id in ["rate", "volume", "duck"] {
            let tag = format!("<input id=\"{id}\" type=\"range\"");
            let at = PAGE.find(&tag).unwrap_or_else(|| panic!("{tag}"));
            let end = at + PAGE[at..].find('>').unwrap();
            let input = &PAGE[at..end];
            for attr in ["min=", "max=", "value="] {
                assert!(!input.contains(attr), "{input}");
            }
        }
    }

    #[test]
    fn base64_matches_rfc_4648() {
        for (raw, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(raw.as_bytes()), enc, "{raw:?}");
        }
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn the_page_carries_its_fonts_inline_and_nothing_external() {
        assert_eq!(PAGE.matches(FONTS_SLOT).count(), 1);
        let page = render(Some("127.0.0.1:5000"), Some("token=abc123"), 5000, "abc123").unwrap();
        assert!(!page.contains(FONTS_SLOT));
        assert_eq!(page.matches("@font-face").count(), FONTS.len());
        assert!(page.contains("font-family:\"Geist Mono\""));
        // Only data: URLs: the CSP forbids any other origin anyway.
        for needle in ["http://", "https://"] {
            for (i, _) in page.match_indices(needle) {
                let before = &page[i.saturating_sub(12)..i];
                assert!(
                    !before.contains("url(") && !before.contains("src="),
                    "an external resource at {i}"
                );
            }
        }
        let csp = HEADERS
            .iter()
            .find(|(k, _)| *k == "content-security-policy")
            .unwrap()
            .1;
        assert!(csp.contains("font-src data:"));
        // Each font is a whole woff2 file.
        for (_, woff2) in FONTS {
            assert_eq!(&woff2[..4], b"wOF2");
        }
    }
}
