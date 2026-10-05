//! Strip markdown noise and normalize symbols so text reads naturally aloud.
//! Port of the retired Python plugin's `cleaner.py` (removed in #248); the
//! golden fixtures (`tests/fixtures/text_rules`) are the contract.
use once_cell::sync::Lazy;
use regex::Regex;

// Python's `\s` on str also matches the separators U+001C..U+001F (they count as
// str.isspace()); Rust's Unicode `\s` does not. Every whitespace class below is
// written as `[\s\x1C-\x1F]` (and `\S` as `[^\s\x1C-\x1F]`) so the rules match
// cleaner.py byte for byte on any input, not only on the golden cases.
//
// cleaner.py's three backtracking rules (_EMPHASIS with a backreference,
// _BARE_URL with a lookahead, _SNAKE with lookbehind and lookahead) are linear
// scanners below instead of fancy-regex: fancy-regex stops after 1M backtracks,
// and its `replace_all` then panics on long input that Python handles.

static LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]\n]+)\]\((?:[^)\n]+)\)").unwrap());
static INLINE_CODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`\n]*)`").unwrap());
static HEADING: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^#{1,6}[\s\x1C-\x1F]+").unwrap());
static BULLET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^([\s\x1C-\x1F]*)[-*+•][ \t]+").unwrap());
static URL_SCHEME: Lazy<Regex> = Lazy::new(|| Regex::new(r"https?://").unwrap());
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
static WORD_RUN: Lazy<Regex> = Lazy::new(|| Regex::new(r"[A-Za-z0-9_]+").unwrap());
static ARROW: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[\s\x1C-\x1F]*(?:->|=>|-->|→)[\s\x1C-\x1F]*").unwrap());
static STRAY_MD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[*_`~#|•✓✔✗✘❌✅]+").unwrap());

/// Length-preserving "N. " -> "N: " for numbered list items (raw text, pre-split).
pub fn stabilize_ordinals(text: &str) -> String {
    LIST_ORDINAL.replace_all(text, "${1}${2}:${3}").into_owned()
}

/// Python `str.isspace()`: Unicode White_Space plus U+001C..U+001F.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// One pass of `(\*{1,3}|_{1,3})([^*_\n]+)\1` -> `\2`, as Python's re runs it:
/// one alternation with a backreference, left to right.
///
/// At a marker the greedy `{1,3}` can only succeed with the whole marker run
/// (a shorter one leaves a marker where the content must start), so a run of
/// 4 or more never matches there. The greedy content can only close at its
/// full length (a shorter one leaves a content char where `\1` must be). Each
/// start position therefore has one candidate, checked without backtracking.
fn emphasis_pass(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut last, mut p) = (0, 0);
    while p < b.len() {
        let m = b[p];
        if m != b'*' && m != b'_' {
            p += 1;
            continue;
        }
        let mut run = 0;
        while run < 4 && p + run < b.len() && b[p + run] == m {
            run += 1;
        }
        if run <= 3 {
            let cs = p + run;
            let mut ce = cs;
            while ce < b.len() && !matches!(b[ce], b'*' | b'_' | b'\n') {
                ce += 1;
            }
            if ce > cs && ce + run <= b.len() && b[ce..ce + run].iter().all(|&x| x == m) {
                // markers are ASCII, so p, cs, ce and ce + run are char boundaries
                out.push_str(&text[last..p]);
                out.push_str(&text[cs..ce]);
                last = ce + run;
                p = last;
                continue;
            }
        }
        p += 1;
    }
    out.push_str(&text[last..]);
    out
}

/// `https?://\S+?(?=[.,;:!?)\]]*(?:\s|$))` -> "link".
///
/// The lazy body is the shortest (at least one char) prefix of the non-space
/// token after the scheme whose remainder is all closing punctuation: the
/// token minus its trailing punctuation, or its first char when that is empty.
fn replace_bare_urls(text: &str) -> String {
    const PUNCT: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']'];
    let mut out = String::with_capacity(text.len());
    let (mut last, mut pos) = (0, 0);
    while let Some(m) = URL_SCHEME.find_at(text, pos) {
        let rest = &text[m.end()..];
        let token = &rest[..rest.find(is_py_space).unwrap_or(rest.len())];
        let Some(first) = token.chars().next() else {
            pos = m.start() + 1; // 'h' is ASCII
            continue;
        };
        let body = token.trim_end_matches(PUNCT).len().max(first.len_utf8());
        out.push_str(&text[last..m.start()]);
        out.push_str("link");
        last = m.end() + body;
        pos = last;
    }
    out.push_str(&text[last..]);
    out
}

/// `(?<![A-Za-z0-9_])_{0,2}[A-Za-z][A-Za-z0-9]*(?:_[A-Za-z0-9]+)+(?![A-Za-z0-9_])`
/// with every `_` of the match spoken as a space.
///
/// Every char the pattern consumes is in `[A-Za-z0-9_]` and both lookarounds
/// reject that class, so a match is always one whole maximal run of it. The
/// run qualifies when, after at most two leading underscores, it starts with
/// a letter and is 2+ non-empty alphanumeric parts joined by single `_`.
fn speak_snake_case(text: &str) -> String {
    fn is_snake(run: &str) -> bool {
        let rest = run.trim_start_matches('_');
        if run.len() - rest.len() > 2 || !rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return false;
        }
        let mut parts = 0;
        for part in rest.split('_') {
            if part.is_empty() {
                return false;
            }
            parts += 1;
        }
        parts >= 2
    }
    WORD_RUN
        .replace_all(text, |c: &regex::Captures| {
            if is_snake(&c[0]) {
                c[0].replace('_', " ")
            } else {
                c[0].to_string()
            }
        })
        .into_owned()
}

pub fn clean_markdown(text: &str) -> String {
    let mut t = LINK.replace_all(text, "${1}").into_owned();
    t = INLINE_CODE.replace_all(&t, "${1}").into_owned();
    t = HEADING.replace_all(&t, "").into_owned();
    t = BULLET.replace_all(&t, "${1}").into_owned();
    t = emphasis_pass(&t);
    t = emphasis_pass(&t);
    t = replace_bare_urls(&t);
    t = TABLE_SEP.replace_all(&t, " ").into_owned();
    t = stabilize_ordinals(&t);
    t = LIST_ITEM_END.replace_all(&t, "${1}.\n").into_owned();
    t = WHITESPACE.replace_all(&t, " ").into_owned();
    t.trim().to_string()
}

pub fn normalize_for_speech(text: &str) -> String {
    let mut t = clean_markdown(&speak_snake_case(text));
    t = ARROW.replace_all(&t, " to ").into_owned();
    t = t.replace(" & ", " and ");
    t = STRAY_MD.replace_all(&t, " ").into_owned();
    WHITESPACE.replace_all(&t, " ").trim().to_string()
}

#[cfg(test)]
pub(crate) mod tests {
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

    /// Inputs that exceeded fancy-regex's backtrack limit or stack (#170 review).
    pub(crate) fn long_inputs() -> Vec<String> {
        vec![
            "word ".repeat(400_000),
            format!("*{}", "a".repeat(1_100_000)),
            "a_".repeat(300_000),
            format!("see https://x.y/{} now", "q".repeat(600_000)),
        ]
    }

    #[test]
    fn long_input_never_panics() {
        let inputs = long_inputs();
        assert_eq!(
            normalize_for_speech(&inputs[0]),
            "word ".repeat(400_000).trim_end()
        );
        assert_eq!(normalize_for_speech(&inputs[1]), "a".repeat(1_100_000));
        let _ = normalize_for_speech(&inputs[2]);
        assert_eq!(normalize_for_speech(&inputs[3]), "see link now");
    }
}
