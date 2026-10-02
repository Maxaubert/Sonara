//! Strip markdown noise and normalize symbols so text reads naturally aloud.
//! Port of src/sonara/cleaner.py; the golden fixtures are the contract.
use fancy_regex::Regex as FRegex;
use once_cell::sync::Lazy;
use regex::Regex;

// Python's `\s` on str also matches the separators U+001C..U+001F (they count as
// str.isspace()); Rust's Unicode `\s` does not. Every whitespace class below is
// written as `[\s-]` (and `\S` as `[^\s-]`) so the rules match
// cleaner.py byte for byte on any input, not only on the golden cases.

static LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]\n]+)\]\((?:[^)\n]+)\)").unwrap());
static INLINE_CODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`\n]*)`").unwrap());
static HEADING: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^#{1,6}[\s\x1C-\x1F]+").unwrap());
static BULLET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^([\s\x1C-\x1F]*)[-*+•][ \t]+").unwrap());
// Python applies one alternation with a backreference, left to right per pass.
// Rust `regex` has no backreferences; per-marker patterns diverge on mixed runs
// such as "---_?!~:9___[___", so this uses fancy-regex's exact equivalent.
static EMPHASIS: Lazy<FRegex> = Lazy::new(|| FRegex::new(r"(\*{1,3}|_{1,3})([^*_\n]+)\1").unwrap());
static BARE_URL: Lazy<FRegex> = Lazy::new(|| {
    FRegex::new(r"https?://[^\s\x1C-\x1F]+?(?=[.,;:!?)\]]*(?:[\s\x1C-\x1F]|$))").unwrap()
});
static TABLE_SEP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^[\s\x1C-\x1F]*\|?[\s\x1C-\x1F:|-]*-{3,}[\s\x1C-\x1F:|-]*\|?[\s\x1C-\x1F]*$")
        .unwrap()
});
static LIST_ORDINAL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^([\s\x1C-\x1F]*)(\d{1,3})\.([\s\x1C-\x1F]+)").unwrap());
static LIST_ITEM_END: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^([\s\x1C-\x1F]*\d{1,3}: .*[^\s\x1C-\x1F.!?:;])[ \t]*\n").unwrap()
});
static WHITESPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\s\x1C-\x1F]+").unwrap());
static SNAKE: Lazy<FRegex> = Lazy::new(|| {
    FRegex::new(r"(?<![A-Za-z0-9_])_{0,2}[A-Za-z][A-Za-z0-9]*(?:_[A-Za-z0-9]+)+(?![A-Za-z0-9_])")
        .unwrap()
});
static ARROW: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[\s\x1C-\x1F]*(?:->|=>|-->|→)[\s\x1C-\x1F]*").unwrap());
static STRAY_MD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[*_`~#|•✓✔✗✘❌✅]+").unwrap());

/// Length-preserving "N. " -> "N: " for numbered list items (raw text, pre-split).
pub fn stabilize_ordinals(text: &str) -> String {
    LIST_ORDINAL.replace_all(text, "${1}${2}:${3}").into_owned()
}

fn emphasis_pass(text: &str) -> String {
    EMPHASIS.replace_all(text, "${2}").into_owned()
}

pub fn clean_markdown(text: &str) -> String {
    let mut t = LINK.replace_all(text, "${1}").into_owned();
    t = INLINE_CODE.replace_all(&t, "${1}").into_owned();
    t = HEADING.replace_all(&t, "").into_owned();
    t = BULLET.replace_all(&t, "${1}").into_owned();
    t = emphasis_pass(&t);
    t = emphasis_pass(&t);
    t = BARE_URL.replace_all(&t, "link").into_owned();
    t = TABLE_SEP.replace_all(&t, " ").into_owned();
    t = stabilize_ordinals(&t);
    t = LIST_ITEM_END.replace_all(&t, "${1}.\n").into_owned();
    t = WHITESPACE.replace_all(&t, " ").into_owned();
    t.trim().to_string()
}

pub fn normalize_for_speech(text: &str) -> String {
    let t = SNAKE
        .replace_all(text, |c: &fancy_regex::Captures| c[0].replace('_', " "))
        .into_owned();
    let mut t = clean_markdown(&t);
    t = ARROW.replace_all(&t, " to ").into_owned();
    t = t.replace(" & ", " and ");
    t = STRAY_MD.replace_all(&t, " ").into_owned();
    WHITESPACE.replace_all(&t, " ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinals_are_length_preserving() {
        let s = "1. a\n  22. b";
        assert_eq!(stabilize_ordinals(s), "1: a\n  22: b");
        assert_eq!(stabilize_ordinals(s).len(), s.len());
    }

    #[test]
    fn snake_case_and_arrows() {
        assert_eq!(
            normalize_for_speech("call get_user_id -> done & ok"),
            "call get user id to done and ok"
        );
    }

    #[test]
    fn bare_url_keeps_terminator() {
        assert_eq!(
            clean_markdown("See https://x.y/z. Next."),
            "See link. Next."
        );
    }

    #[test]
    fn python_whitespace_and_emphasis_quirks() {
        // U+001C..U+001F are whitespace to Python's re and str.strip().
        assert_eq!(clean_markdown("\u{1c}"), "");
        assert_eq!(stabilize_ordinals("\u{1c}1. a"), "\u{1c}1: a");
        // One backreference alternation, left to right, as in cleaner.py.
        assert_eq!(clean_markdown("---_?!~:9___[___"), "---?!~:9[_");
    }

    #[test]
    fn unicode_text_never_panics() {
        for s in ["→ 🚀 “quoted” 日本語 **bold** é", "_", "`", "1.", ""] {
            let _ = normalize_for_speech(s);
            let _ = clean_markdown(s);
        }
    }
}
