//! Secret masking for log lines (#219): a log may carry hook payloads and
//! protocol messages, and those may hold a command line with a key in it.
//!
//! - `secret_key(name)`: a field whose name says it holds a secret
//!   (`api_key`, `password`, `Authorization`, `Ocp-Apim-Subscription-Key`, ...). Its whole value is
//!   masked by the caller.
//! - `mask(text)`: masks credential-looking words inside free text: known
//!   token prefixes (`sk-`, `sk_car_`, `ghp_`, `github_pat_`, `AKIA`, `xoxb-`, JWTs),
//!   the word after `Bearer`/`Basic`, and the value of an assignment whose
//!   name is a `secret_key` (`PASSWORD=...`, `"api_key": "..."`).
//!
//! Best effort: it catches the common shapes, not every secret.
use std::borrow::Cow;

/// What a masked value becomes.
pub const MASK: &str = "[redacted]";

/// Name fragments (lower case) of a field that holds a secret.
const SECRET_NAMES: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "api_key",
    "apikey",
    "api-key",
    "authorization",
    "cookie",
    "credential",
    "private_key",
    "access_key",
    "client_secret",
    // Speech providers' key headers (external engines, #224).
    "xi-api-key",
    "subscription-key",
    "x-goog-api-key",
];

/// Prefixes of well-known credential formats (case sensitive).
const TOKEN_PREFIXES: &[&str] = &[
    "sk-",
    // Cartesia.
    "sk_car_",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "AKIA",
    "ASIA",
    "AIza",
    "eyJ",
];

/// A credential-looking word is at least this long.
const MIN_TOKEN: usize = 16;

/// Whether a field named `name` holds a secret.
pub fn secret_key(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_NAMES.iter().any(|s| lower.contains(s))
}

fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+' | '=' | '~')
}

fn looks_like_token(w: &str) -> bool {
    w.len() >= MIN_TOKEN && TOKEN_PREFIXES.iter().any(|p| w.starts_with(p))
}

#[derive(Clone, Copy)]
enum Next {
    No,
    Loose,
    Strict,
}

/// `text` with credential-looking words masked (module docs).
pub fn mask(text: &str) -> Cow<'_, str> {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    // How the next word is masked: `Loose` after `Bearer` or a `--password`
    // flag (spaces may separate it), `Strict` after a secret name only when
    // `:` or `=` follows it (`"api_key": "abc"`, not "the token count").
    let mut next = Next::No;
    let mut rest = text;
    while !rest.is_empty() {
        let start = rest.find(word_char).unwrap_or(rest.len());
        let gap = &rest[..start];
        out.push_str(gap);
        rest = &rest[start..];
        if rest.is_empty() {
            break;
        }
        let end = rest.find(|c| !word_char(c)).unwrap_or(rest.len());
        let word = &rest[..end];
        rest = &rest[end..];
        let quiet = gap
            .chars()
            .all(|c| matches!(c, '"' | '\'' | ':' | '=' | ' ' | '\t'));
        let follows = match next {
            Next::No => false,
            Next::Loose => quiet,
            Next::Strict => quiet && gap.contains([':', '=']),
        };
        next = Next::No;
        if word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic") {
            out.push_str(word);
            next = Next::Loose;
            continue;
        }
        // An assignment written as one word: `PASSWORD=hunter2`, `K=sk-...`.
        if let Some((name, value)) = word.split_once('=') {
            if !value.is_empty() && (secret_key(name) || looks_like_token(value)) {
                out.push_str(name);
                out.push('=');
                out.push_str(MASK);
                changed = true;
                continue;
            }
        }
        if follows || looks_like_token(word) {
            out.push_str(MASK);
            changed = true;
            continue;
        }
        out.push_str(word);
        if secret_key(word) {
            next = if word.starts_with('-') {
                Next::Loose
            } else {
                Next::Strict
            };
        }
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_field_names_are_recognised() {
        for n in [
            "api_key",
            "OPENAI_API_KEY",
            "Authorization",
            "password",
            "x-auth-token",
        ] {
            assert!(secret_key(n), "{n}");
        }
        for n in ["session_id", "tool_use_id", "prompt_id", "message", "cwd"] {
            assert!(!secret_key(n), "{n}");
        }
    }

    #[test]
    fn credential_looking_words_are_masked() {
        let key = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        assert_eq!(mask(&format!("export K={key}")), "export K=[redacted]");
        assert_eq!(mask(&format!("use {key} here")), "use [redacted] here");
        assert_eq!(
            mask("curl -H 'Authorization: Bearer abc.def.ghi' x"),
            "curl -H 'Authorization: Bearer [redacted]' x"
        );
        assert_eq!(
            mask("GITHUB_TOKEN=ghp_1234567890abcdef1234 gh pr list"),
            "GITHUB_TOKEN=[redacted] gh pr list"
        );
        assert_eq!(
            mask("set PASSWORD=hunter2 now"),
            "set PASSWORD=[redacted] now"
        );
        assert_eq!(
            mask(r#"{"api_key": "abc123"}"#),
            r#"{"api_key": "[redacted]"}"#
        );
        assert_eq!(
            mask("login --password hunter2"),
            "login --password [redacted]"
        );
    }

    #[test]
    fn speech_provider_keys_and_headers_are_masked() {
        // Spec docs/plans/2026-10-04-external-engines-spec.md 4.3.
        for n in [
            "secret",
            "xi-api-key",
            "Ocp-Apim-Subscription-Key",
            "X-Goog-Api-Key",
        ] {
            assert!(secret_key(n), "{n}");
        }
        assert_eq!(
            mask("cartesia key sk_car_abcdefghijklmnop1234 here"),
            "cartesia key [redacted] here"
        );
        assert_eq!(
            mask("Ocp-Apim-Subscription-Key: 0123456789abcdef"),
            "Ocp-Apim-Subscription-Key: [redacted]"
        );
        assert_eq!(
            mask("sk-proj-abcdefghijklmnopqrstuvwx"),
            "[redacted]",
            "OpenAI project keys are covered by sk-"
        );
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for t in [
            "Claude needs your permission",
            "cargo test --workspace",
            "Which color do you want? Red, Blue",
            "the token count is fine",
        ] {
            assert!(matches!(mask(t), Cow::Borrowed(_)), "{t}");
        }
    }
}
