//! Split a chunk that is longer than a provider's input limit (spec 13.1
//! "Input limits"). A reader chunk is one sentence, or a whole message cut
//! to the limit at paragraphs and sentences (send mode `message`, #235), so
//! this runs only for a single sentence over the limit: parts end at
//! spaces, and only a single word longer than the limit is cut inside.

/// A provider's input limit (the engine-wide `InputLimit`).
pub use crate::InputLimit as Limit;

/// Cut one word into pieces of at most `limit`, at character boundaries.
fn cut_word(word: &str, limit: Limit, out: &mut Vec<String>) {
    let mut cur = String::new();
    for c in word.chars() {
        let mut next = cur.clone();
        next.push(c);
        if !cur.is_empty() && limit.size(&next) > limit.max() {
            out.push(std::mem::take(&mut cur));
            cur.push(c);
        } else {
            cur = next;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
}

/// The parts of `text`, each within `limit`, in order. Text within the
/// limit is one part, unchanged.
pub fn split(text: &str, limit: Limit) -> Vec<String> {
    if limit.size(text) <= limit.max() {
        return vec![text.to_string()];
    }
    let mut parts = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if limit.size(word) > limit.max() {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            cut_word(word, limit, &mut parts);
            continue;
        }
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if limit.size(&candidate) > limit.max() {
            parts.push(std::mem::replace(&mut cur, word.to_string()));
        } else {
            cur = candidate;
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_part() {
        assert_eq!(
            split("Hello there.", Limit::Chars(4096)),
            vec!["Hello there."]
        );
        assert_eq!(split("", Limit::Chars(5)), vec![""]);
    }

    #[test]
    fn parts_end_at_spaces() {
        let parts = split("one two three four five", Limit::Chars(9));
        assert_eq!(parts, vec!["one two", "three", "four five"]);
        assert!(parts.iter().all(|p| p.chars().count() <= 9));
    }

    #[test]
    fn a_word_longer_than_the_limit_is_cut() {
        assert_eq!(
            split("ab abcdefgh cd", Limit::Chars(3)),
            vec!["ab", "abc", "def", "gh", "cd"]
        );
    }

    #[test]
    fn chars_and_bytes_differ_for_cjk_and_emoji() {
        let cjk = "日本語の文章です 次の文章";
        // 13 characters, 37 bytes.
        assert_eq!(split(cjk, Limit::Chars(20)).len(), 1);
        let by_bytes = split(cjk, Limit::Bytes(24));
        assert_eq!(by_bytes, vec!["日本語の文章です", "次の文章"]);
        assert!(by_bytes.iter().all(|p| p.len() <= 24));
        // A word of emoji cut by bytes never splits a character.
        let emoji = "😀😀😀😀";
        let parts = split(emoji, Limit::Bytes(9));
        assert_eq!(parts, vec!["😀😀", "😀😀"]);
        assert_eq!(parts.concat(), emoji);
    }
}
